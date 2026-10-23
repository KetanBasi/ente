// fork: tests for the Home AI client (x_enrich parsing, suggestions, search
// matches and key release).

import type { EnteFile } from "ente-media/file";
import {
    enrichFromMLData,
    type XEnrich,
} from "ente-new/photos/services/fork/enrich";
import { sampleEnrich } from "ente-new/photos/services/fork/samples";
import {
    computeSuggestions,
    defaultHomeAISettings,
    entityMatches,
} from "ente-new/photos/services/fork/suggestions";
import { beforeEach, describe, expect, test, vi } from "vitest";

const kv = vi.hoisted(() => new Map<string, unknown>());
vi.mock("ente-base/kv", () => ({
    getKV: (k: string) => Promise.resolve(kv.get(k)),
    setKV: (k: string, v: unknown) => Promise.resolve(void kv.set(k, v)),
    removeKV: (k: string) => Promise.resolve(void kv.delete(k)),
}));

const api = vi.hoisted(() => ({
    fetchWorkerKeys: vi.fn(),
    fetchWorkerStatus: vi.fn(),
    fetchAuditLog: vi.fn(),
    releaseKeys: vi.fn(),
}));
vi.mock("ente-new/photos/services/fork/worker-api", () => ({
    ...api,
    maxInboxBatch: 2,
}));

vi.mock("ente-base/crypto", () => ({
    fromB64: (s: string) =>
        Promise.resolve(new Uint8Array(Buffer.from(s, "base64"))),
    boxSeal: (data: string, publicKey: string) =>
        Promise.resolve(`sealed(${data},${publicKey})`),
}));

const {
    releaseKeysIfNeeded,
    saveDeviceWorker,
    savedDeviceWorker,
    workerFingerprint,
} = await import("ente-new/photos/services/fork/key-release");

const enrich = (e: Partial<XEnrich> = {}): XEnrich => ({
    v: 1,
    client: "test",
    at: 1,
    ...e,
});

const file = (id: number, review?: Record<string, string>) =>
    ({
        id,
        key: `key${id}`,
        magicMetadata: review
            ? { version: 1, count: 1, data: { x_review: review } }
            : undefined,
    }) as unknown as EnteFile;

describe("enrichFromMLData", () => {
    test("reads a valid record and keeps Ente's own keys out of it", () => {
        const e = enrich({
            album: { name: "Receipts", score: 0.9 },
            languages: ["en", "ja"],
        });
        expect(enrichFromMLData({ face: {}, clip: {}, x_enrich: e })).toEqual(
            e,
        );
    });

    test("ignores records without x_enrich or of an unknown version", () => {
        expect(enrichFromMLData({ face: {} })).toBeUndefined();
        expect(
            enrichFromMLData({ x_enrich: { ...enrich(), v: 2 } }),
        ).toBeUndefined();
    });

    test("drops a malformed field but keeps the rest", () => {
        const parsed = enrichFromMLData({
            x_enrich: {
                ...enrich({ languages: ["en"] }),
                pii: { score: 7, kinds: "card" },
                entities: [
                    { kind: "repo", name: "ente-io/ente", score: 0.9 },
                    { kind: "repo", name: "", score: 0.9 },
                    "junk",
                ],
            },
        });
        expect(parsed?.pii).toBeUndefined();
        expect(parsed?.languages).toEqual(["en"]);
        expect(parsed?.entities).toEqual([
            { kind: "repo", name: "ente-io/ente", score: 0.9 },
        ]);
    });

    test("sample results are valid x_enrich records", () => {
        const samples = sampleEnrich(
            [1, 2, 3, 12, 15, 18, 21].map((id) => file(id)),
        );
        expect(samples.size).toBeGreaterThan(0);
        for (const e of samples.values()) {
            expect(enrichFromMLData({ x_enrich: e })).toEqual(e);
        }
    });
});

describe("computeSuggestions", () => {
    const settings = defaultHomeAISettings;

    test("groups album suggestions and skips small, unsure or reviewed ones", () => {
        const files = [1, 2, 3, 4, 5, 6].map((id) =>
            file(id, id == 3 ? { album: "rejected" } : undefined),
        );
        const byID = new Map<number, XEnrich>([
            [1, enrich({ album: { name: "Trips", score: 0.9 } })],
            [2, enrich({ album: { name: "Trips", score: 0.8 } })],
            [3, enrich({ album: { name: "Trips", score: 0.9 } })],
            [4, enrich({ album: { name: "Trips", score: 0.1 } })],
            [5, enrich({ album: { name: "Alone", score: 0.9 } })],
        ]);
        const { albums } = computeSuggestions(files, byID, [], settings);
        expect(albums).toHaveLength(1);
        expect(albums[0]!.name).toBe("Trips");
        expect(albums[0]!.files.map((f) => f.id)).toEqual([1, 2]);
        expect(albums[0]!.existingCollectionID).toBeUndefined();
    });

    test("offers an existing album without the files already in it", () => {
        const files = [1, 2, 3].map((id) => file(id));
        const byID = new Map(
            files.map((f) => [
                f.id,
                enrich({ album: { name: "trips ", score: 0.9 } }),
            ]),
        );
        const { albums } = computeSuggestions(
            files,
            byID,
            [{ id: 77, name: "Trips", fileIDs: new Set([1]) }],
            settings,
        );
        expect(albums[0]!.existingCollectionID).toBe(77);
        expect(albums[0]!.files.map((f) => f.id)).toEqual([2, 3]);
    });

    test("sorts cleanup and hide suggestions by score and respects choices", () => {
        const files = [1, 2, 3, 4].map((id) =>
            file(
                id,
                id == 4 ? { cleanup: "keep", pii: "dismissed" } : undefined,
            ),
        );
        const byID = new Map<number, XEnrich>([
            [1, enrich({ cleanup: { score: 0.75, reasons: ["blur"] } })],
            [2, enrich({ cleanup: { score: 0.95, reasons: ["duplicate"] } })],
            [3, enrich({ pii: { score: 0.4, kinds: ["iban"] } })],
            [
                4,
                enrich({
                    cleanup: { score: 0.99, reasons: ["blur"] },
                    pii: { score: 0.99, kinds: ["passport"] },
                }),
            ],
        ]);
        const { cleanup, hide } = computeSuggestions(files, byID, [], settings);
        expect(cleanup.map((s) => s.file.id)).toEqual([2, 1]);
        expect(cleanup[0]!.labels).toEqual(["duplicate"]);
        expect(hide).toEqual([]);
        expect(
            computeSuggestions(files, byID, [], {
                ...settings,
                minPIIScore: 0.3,
            }).hide.map((s) => s.file.id),
        ).toEqual([3]);
    });
});

describe("entityMatches", () => {
    const byID = new Map<number, XEnrich>([
        [
            1,
            enrich({
                entities: [{ kind: "repo", name: "ente-io/ente", score: 1 }],
                ocr: { text: "Star ente-io/ente on GitHub" },
            }),
        ],
        [
            2,
            enrich({
                entities: [{ kind: "repo", name: "ente-io/ente", score: 1 }],
                device: { make: "Google", model: "Pixel 8" },
            }),
        ],
    ]);

    test("groups entities by name, then devices, then OCR text", () => {
        expect(entityMatches("ente", "Ente", byID)).toEqual([
            { label: "ente-io/ente", kind: "repo", fileIDs: [1, 2] },
            { label: "Ente", kind: "text", fileIDs: [1] },
        ]);
        expect(entityMatches("pixel", "Pixel", byID)).toEqual([
            { label: "Google Pixel 8", kind: "device", fileIDs: [2] },
        ]);
    });

    test("needs at least two characters, and three for OCR text", () => {
        expect(entityMatches("e", "e", byID)).toEqual([]);
        expect(entityMatches("gi", "gi", byID)).toEqual([]);
    });
});

describe("releaseKeysIfNeeded", () => {
    // 32 bytes, base64.
    const publicKey = Buffer.alloc(32, 7).toString("base64");
    const key = { name: "home-pc", publicKey, fingerprint: undefined };
    const status = (tokenID: string) => ({
        workerPublicKey: publicKey,
        createdAt: 1,
        lastUsedAt: undefined,
        pendingCount: 0,
        consumedCount: 0,
        tokenID,
        workerName: "home-pc",
    });

    beforeEach(() => {
        kv.clear();
        vi.resetAllMocks();
        api.fetchWorkerKeys.mockResolvedValue([key]);
        api.fetchAuditLog.mockResolvedValue([]);
        api.releaseKeys.mockResolvedValue(0);
    });

    test("does nothing until the user confirms a worker on this device", async () => {
        expect(await releaseKeysIfNeeded([file(1)], [status("t")])).toEqual({
            state: "off",
        });
        expect(api.releaseKeys).not.toHaveBeenCalled();
    });

    test("computes the fingerprint locally", async () => {
        expect(await workerFingerprint(publicKey)).toMatch(/^[0-9a-f]{64}$/);
        await expect(workerFingerprint("AAAA")).rejects.toThrow();
    });

    test("seals only files the worker does not have, in batches", async () => {
        await saveDeviceWorker(key, await workerFingerprint(publicKey));
        api.fetchAuditLog
            .mockResolvedValueOnce([
                {
                    id: 5,
                    fileID: 1,
                    workerPublicKey: publicKey,
                    createdAt: 1,
                    consumedAt: undefined,
                },
                {
                    id: 6,
                    fileID: 2,
                    workerPublicKey: "other",
                    createdAt: 1,
                    consumedAt: undefined,
                },
            ])
            .mockResolvedValueOnce([]);

        const result = await releaseKeysIfNeeded(
            [1, 2, 3, 4].map((id) => file(id)),
            [status("t1")],
        );

        expect(result).toEqual({ state: "done", released: 3, remaining: 0 });
        expect(api.releaseKeys.mock.calls).toEqual([
            [
                publicKey,
                [
                    { fileID: 2, sealedFileKey: `sealed(key2,${publicKey})` },
                    { fileID: 3, sealedFileKey: `sealed(key3,${publicKey})` },
                ],
            ],
            [
                publicKey,
                [{ fileID: 4, sealedFileKey: `sealed(key4,${publicKey})` }],
            ],
        ]);
        expect((await savedDeviceWorker())?.auditCursor).toBe(6);

        // A second sync with the same token sends nothing more.
        api.releaseKeys.mockClear();
        await releaseKeysIfNeeded(
            [1, 2, 3, 4].map((id) => file(id)),
            [status("t1")],
        );
        expect(api.releaseKeys).not.toHaveBeenCalled();

        // A new token (kill switch, then enable) restarts from the log.
        await releaseKeysIfNeeded([file(1)], [status("t2")]);
        expect(api.releaseKeys).toHaveBeenCalledOnce();
    });

    test("stops and forgets the worker if its key changed on the server", async () => {
        await saveDeviceWorker(key, "not-the-real-fingerprint");
        const result = await releaseKeysIfNeeded([file(1)], [status("t")]);
        expect(result.state).toBe("stopped");
        expect(api.releaseKeys).not.toHaveBeenCalled();
        expect(await savedDeviceWorker()).toBeUndefined();
    });

    test("stops if Home AI was turned off for the account", async () => {
        await saveDeviceWorker(key, await workerFingerprint(publicKey));
        const result = await releaseKeysIfNeeded([file(1)], []);
        expect(result.state).toBe("stopped");
        expect(await savedDeviceWorker()).toBeUndefined();
    });
});
