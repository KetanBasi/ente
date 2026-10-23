// fork: made-up `x_enrich` results, so the Home AI screens can be tried
// before the worker exists. They stay on this device and are never uploaded.

import type { EnteFile } from "ente-media/file";
import type { EnrichEntity, XEnrich } from "./enrich";

const sampleClient = "ente-fork-sample/1";

const albums = ["Open source", "Anime screenshots", "Receipts"];

const entities: EnrichEntity[][] = [
    [{ kind: "repo", name: "ente-io/ente", score: 0.97 }],
    [{ kind: "social", name: "@ente_io", score: 0.9 }],
    [{ kind: "anime", name: "Frieren: Beyond Journey's End", score: 0.82 }],
    [{ kind: "movie", name: "Spirited Away", score: 0.76 }],
];

/**
 * Give roughly every third file some made-up results. The choice is a pure
 * function of the file ID, so loading samples twice gives the same result.
 */
export const sampleEnrich = (files: EnteFile[]): Map<number, XEnrich> => {
    const result = new Map<number, XEnrich>();
    for (const file of files) {
        const n = file.id % 12;
        if (n % 3 != 0) continue;
        const enrich: XEnrich = {
            v: 1,
            client: sampleClient,
            at: Date.now() * 1000,
            languages: ["en"],
        };
        switch (n) {
            case 0:
                enrich.album = { name: albums[0]!, score: 0.8 };
                enrich.entities = entities[0];
                enrich.ocr = { text: "github.com/ente-io/ente README" };
                break;
            case 3:
                enrich.album = { name: albums[1]!, score: 0.7 };
                enrich.entities = entities[2];
                enrich.cleanup = { score: 0.86, reasons: ["blur"] };
                break;
            case 6:
                enrich.album = { name: albums[2]!, score: 0.65 };
                enrich.pii = { score: 0.74, kinds: ["card_number"] };
                enrich.ocr = { text: "VISA **** 4242 total 12.40" };
                break;
            default:
                enrich.entities = [...entities[1]!, ...entities[3]!];
                enrich.cleanup = { score: 0.92, reasons: ["duplicate"] };
                enrich.device = {
                    make: "Google",
                    model: "Pixel 8",
                    kind: "screenshot",
                };
        }
        result.set(file.id, enrich);
    }
    return result;
};
