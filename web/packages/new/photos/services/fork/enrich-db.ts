// fork: a local copy of the worker's `x_enrich` results, kept in its own
// IndexedDB so the review screen and search need no network.

import { getKVN, removeKV, setKV } from "ente-base/kv";
import log from "ente-base/log";
import { syncUpdatedFileDataFileIDs } from "ente-gallery/services/file-data";
import type { EnteFile } from "ente-media/file";
import { batch } from "ente-utils/array";
import { deleteDB, openDB, type DBSchema } from "idb";
import { fetchMLData } from "../ml/ml-data";
import { enrichFromMLData, type XEnrich } from "./enrich";

interface EnrichRecord {
    fileID: number;
    enrich: XEnrich;
    // Made up locally by "preview with sample data", never uploaded.
    sample?: boolean;
}

interface HomeAIDBSchema extends DBSchema {
    enrich: { key: number; value: EnrichRecord };
}

const dbName = "fork-home-ai";

let _db: ReturnType<typeof openHomeAIDB> | undefined;

const openHomeAIDB = () =>
    openDB<HomeAIDBSchema>(dbName, 1, {
        upgrade(db, oldVersion) {
            if (oldVersion < 1) {
                db.createObjectStore("enrich", { keyPath: "fileID" });
            }
        },
        blocking() {
            void _db?.then((db) => db.close());
            _db = undefined;
        },
        terminated() {
            _db = undefined;
        },
    });

const homeAIDB = () => (_db ??= openHomeAIDB());

// Status-diff cursor for `mldata`, separate from Ente's own ML sync.
const cursorKey = "forkHomeAIEnrichCursor";

// Museum caps one batch `mldata` fetch at this many files.
const fetchBatchSize = 200;

export const savedEnrich = async (): Promise<Map<number, XEnrich>> => {
    const records = await (await homeAIDB()).getAll("enrich");
    return new Map(records.map((r) => [r.fileID, r.enrich]));
};

export const hasSampleEnrich = async () =>
    (await (await homeAIDB()).getAll("enrich")).some((r) => r.sample);

/*
 * Fetch `mldata` that changed since the last sync and keep its `x_enrich`.
 *
 * @param filesByID The user's own files. Records of other files are skipped:
 * only the owner releases keys, so the worker writes only to those.
 *
 * @returns true if any record changed.
 */
export const syncEnrich = async (filesByID: Map<number, EnteFile>) => {
    const changedIDs = new Set<number>();
    const lastUpdatedAt = (await getKVN(cursorKey)) ?? 0;
    await syncUpdatedFileDataFileIDs(
        "mldata",
        lastUpdatedAt,
        async ({ fileIDs, lastUpdatedAt }) => {
            const ids = [...fileIDs].filter((id) => filesByID.has(id));
            for (const b of batch(ids, fetchBatchSize)) {
                const mlData = await fetchMLData(
                    new Map(b.map((id) => [id, filesByID.get(id)!])),
                );
                const db = await homeAIDB();
                const tx = db.transaction("enrich", "readwrite");
                for (const id of b) {
                    const raw = mlData.get(id)?.raw;
                    const enrich = raw && enrichFromMLData(raw);
                    const existing = await tx.store.get(id);
                    if (enrich) {
                        await tx.store.put({ fileID: id, enrich });
                        changedIDs.add(id);
                    } else if (existing && !existing.sample) {
                        await tx.store.delete(id);
                        changedIDs.add(id);
                    }
                }
                await tx.done;
            }
            await setKV(cursorKey, lastUpdatedAt);
        },
    );
    if (changedIDs.size) {
        log.info(`Home AI results changed for ${changedIDs.size} files`);
    }
    return changedIDs.size > 0;
};

export const saveSampleEnrich = async (samples: Map<number, XEnrich>) => {
    const db = await homeAIDB();
    const tx = db.transaction("enrich", "readwrite");
    for (const [fileID, enrich] of samples) {
        // Never cover real worker results with made-up ones.
        if (!(await tx.store.get(fileID))) {
            await tx.store.put({ fileID, enrich, sample: true });
        }
    }
    await tx.done;
};

export const clearSampleEnrich = async () => {
    const db = await homeAIDB();
    const tx = db.transaction("enrich", "readwrite");
    for (const r of await tx.store.getAll()) {
        if (r.sample) await tx.store.delete(r.fileID);
    }
    await tx.done;
};

export const clearEnrichDB = async () => {
    const db = _db;
    _db = undefined;
    if (db) (await db).close();
    await deleteDB(dbName);
    await removeKV(cursorKey);
};
