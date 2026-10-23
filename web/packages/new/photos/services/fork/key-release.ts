// fork: release fileKeys to the home AI worker. On a device where the user
// confirmed the worker's fingerprint, each sync seals the keys of the user's
// files that the worker does not have yet to the worker's public key and
// uploads them to its inbox on museum.

import { boxSeal, fromB64 } from "ente-base/crypto";
import { getKV, removeKV, setKV } from "ente-base/kv";
import log from "ente-base/log";
import type { EnteFile } from "ente-media/file";
import { batch } from "ente-utils/array";
import { z } from "zod";
import {
    fetchAuditLog,
    fetchWorkerKeys,
    fetchWorkerStatus,
    maxInboxBatch,
    releaseKeys,
    type WorkerKey,
    type WorkerStatus,
} from "./worker-api";

// What this device remembers about the worker the user confirmed here.
const DeviceWorker = z.object({
    publicKey: z.string(),
    name: z.string(),
    // The fingerprint this client computed and the user confirmed.
    fingerprint: z.string(),
    confirmedAt: z.number(),
    // The token the released keys belong to; a new one restarts the log.
    tokenID: z.string().optional(),
    // Last audit log row folded into the released set.
    auditCursor: z.number().default(0),
});

export type DeviceWorker = z.infer<typeof DeviceWorker>;

const deviceWorkerKey = "forkHomeAIDeviceWorker";
const releasedKey = "forkHomeAIReleasedFileIDs";

// Keys sealed per sync, so the first run on a big library is spread out.
const maxReleasePerSync = 5000;

/*
 * Hex SHA-256 of the raw public key, the same value museum shows. The client
 * computes it itself so that a tampered server response cannot fake it.
 */
export const workerFingerprint = async (publicKeyB64: string) => {
    const raw = await fromB64(publicKeyB64);
    if (raw.length != 32) throw new Error("Worker key is not 32 bytes");
    const digest = await crypto.subtle.digest("SHA-256", new Uint8Array(raw));
    return [...new Uint8Array(digest)]
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("");
};

export const savedDeviceWorker = async (): Promise<
    DeviceWorker | undefined
> => {
    const result = DeviceWorker.safeParse(await getKV(deviceWorkerKey));
    return result.success ? result.data : undefined;
};

export const saveDeviceWorker = (key: WorkerKey, fingerprint: string) =>
    setKV(deviceWorkerKey, {
        publicKey: key.publicKey,
        name: key.name,
        fingerprint,
        confirmedAt: Date.now(),
        auditCursor: 0,
    } satisfies DeviceWorker);

// Forget the worker on this device. Keys stop being released from here.
export const clearDeviceWorker = async () => {
    await removeKV(deviceWorkerKey);
    await removeKV(releasedKey);
};

const savedReleasedFileIDs = async () =>
    new Set(
        z
            .array(z.number())
            .catch([])
            .parse(await getKV(releasedKey)),
    );

const saveReleasedFileIDs = (ids: Set<number>) => setKV(releasedKey, [...ids]);

export type KeyReleaseResult =
    | { state: "off" }
    // The worker was disabled elsewhere, or its key left the allowlist.
    | { state: "stopped"; reason: string }
    | { state: "done"; released: number; remaining: number };

/*
 * Release the keys of files the worker has not been sent yet.
 *
 * @param files The user's own files, unique by ID. Callers leave out hidden
 * files unless the user asked to include them.
 */
export const releaseKeysIfNeeded = async (
    files: EnteFile[],
    statuses: WorkerStatus[],
): Promise<KeyReleaseResult> => {
    const device = await savedDeviceWorker();
    if (!device) return { state: "off" };

    const status = statuses.find((s) => s.workerPublicKey == device.publicKey);
    if (!status) {
        await clearDeviceWorker();
        return {
            state: "stopped",
            reason: "Home AI was turned off for this account.",
        };
    }

    // Recheck the allowlist and the fingerprint the user confirmed, so a key
    // swapped on the server is never sent anything.
    const key = (await fetchWorkerKeys()).find(
        (k) => k.publicKey == device.publicKey,
    );
    if (
        !key ||
        (await workerFingerprint(key.publicKey)) != device.fingerprint
    ) {
        await clearDeviceWorker();
        return {
            state: "stopped",
            reason: "The confirmed worker key is no longer on the server's allowlist.",
        };
    }

    // A new token means the kill switch ran and dropped unread keys, so
    // rebuild the released set from the log.
    let released = await savedReleasedFileIDs();
    let auditCursor = device.auditCursor;
    if (device.tokenID != status.tokenID) {
        released = new Set();
        auditCursor = 0;
    }
    while (true) {
        const items = await fetchAuditLog(auditCursor);
        if (!items.length) break;
        for (const item of items) {
            if (item.workerPublicKey == device.publicKey) {
                released.add(item.fileID);
            }
            auditCursor = Math.max(auditCursor, item.id);
        }
    }
    await saveReleasedFileIDs(released);
    await setKV(deviceWorkerKey, {
        ...device,
        tokenID: status.tokenID,
        auditCursor,
    } satisfies DeviceWorker);

    const pending = files.filter((f) => !released.has(f.id));
    const now = pending.slice(0, maxReleasePerSync);
    let count = 0;
    for (const b of batch(now, maxInboxBatch)) {
        const keys = await Promise.all(
            b.map(async (f) => ({
                fileID: f.id,
                sealedFileKey: await boxSeal(f.key, device.publicKey),
            })),
        );
        await releaseKeys(device.publicKey, keys);
        for (const f of b) released.add(f.id);
        await saveReleasedFileIDs(released);
        count += b.length;
    }
    if (count) log.info(`Released ${count} file keys to the home AI worker`);

    return {
        state: "done",
        released: count,
        remaining: pending.length - count,
    };
};

// Statuses of this account's enabled workers, or undefined if the server has no Home AI.
export const fetchWorkerStatusIfAvailable = async () => {
    try {
        return await fetchWorkerStatus();
    } catch (e) {
        log.info("Home AI is not available on this server", e);
        return undefined;
    }
};
