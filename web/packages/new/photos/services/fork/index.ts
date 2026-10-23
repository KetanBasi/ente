// fork: Home AI, the client side of the fork's home AI worker. It lets the
// user confirm the worker once, releases fileKeys to it during sync, and turns
// its `x_enrich` results into suggestions and search results. See
// fork/docs/home-ai-client.md.

import { ensureLocalUser } from "ente-accounts/services/user";
import { getKV, removeKV, setKV } from "ente-base/kv";
import log from "ente-base/log";
import { uniqueFilesByID } from "ente-gallery/utils/file";
import type { EnteFile } from "ente-media/file";
import { z } from "zod";
import { savedNormalCollections } from "../collection";
import { computeNormalCollectionFilesFromSaved } from "../file";
import { savedCollectionFiles } from "../photos-fdb";
import type { SearchSuggestion } from "../search/types";
import type { XEnrich } from "./enrich";
import {
    clearEnrichDB,
    clearSampleEnrich,
    hasSampleEnrich,
    savedEnrich,
    saveSampleEnrich,
    syncEnrich,
} from "./enrich-db";
import {
    clearDeviceWorker,
    fetchWorkerStatusIfAvailable,
    releaseKeysIfNeeded,
    savedDeviceWorker,
    saveDeviceWorker,
    workerFingerprint,
    type DeviceWorker,
    type KeyReleaseResult,
} from "./key-release";
import { sampleEnrich } from "./samples";
import {
    computeSuggestions,
    defaultHomeAISettings,
    entityMatches,
    type HomeAISettings,
    type HomeAISuggestions,
} from "./suggestions";
import {
    disableWorkers,
    enableWorker,
    fetchWorkerKeys,
    type WorkerKey,
    type WorkerStatus,
} from "./worker-api";

export interface HomeAIState {
    // False when the server does not have the fork's worker patch.
    available: boolean;
    // The server's worker allowlist.
    keys: WorkerKey[];
    // Workers enabled for this account, from any device.
    workers: WorkerStatus[];
    // The worker this device releases keys to, if the user confirmed one here.
    device: DeviceWorker | undefined;
    settings: HomeAISettings;
    // Files with worker (or sample) results on this device.
    resultCount: number;
    hasSamples: boolean;
    lastRelease: KeyReleaseResult | undefined;
}

let _state: HomeAIState | undefined;
const _listeners = new Set<() => void>();

// Cached results, for search. Cleared whenever the store changes.
let _enrich: Map<number, XEnrich> | undefined;

let _syncPromise: Promise<void> | undefined;

export const homeAISubscribe = (onChange: () => void) => {
    _listeners.add(onChange);
    return () => void _listeners.delete(onChange);
};

export const homeAISnapshot = () => _state;

const setState = (state: HomeAIState) => {
    _state = state;
    _listeners.forEach((l) => l());
};

const settingsKey = "forkHomeAISettings";

const d = defaultHomeAISettings;
const SavedSettings = z
    .object({
        includeHidden: z.boolean().catch(d.includeHidden),
        minAlbumScore: z.number().catch(d.minAlbumScore),
        minCleanupScore: z.number().catch(d.minCleanupScore),
        minPIIScore: z.number().catch(d.minPIIScore),
        search: z.boolean().catch(d.search),
    })
    .catch(d);

const savedSettings = async (): Promise<HomeAISettings> =>
    SavedSettings.parse((await getKV(settingsKey)) ?? {});

export const updateHomeAISettings = async (
    updates: Partial<HomeAISettings>,
) => {
    const settings = { ...(await savedSettings()), ...updates };
    await setKV(settingsKey, settings);
    await pullHomeAIState();
};

const enrichMap = async () => (_enrich ??= await savedEnrich());

/*
 * Refresh {@link homeAISnapshot} from the server and the local store.
 *
 * @param workers Statuses already fetched by the caller, if any.
 */
export const pullHomeAIState = async (
    workers?: WorkerStatus[],
    lastRelease?: KeyReleaseResult,
) => {
    const statuses = workers ?? (await fetchWorkerStatusIfAvailable());
    const available = statuses !== undefined;
    const keys = available ? await fetchWorkerKeys() : [];
    setState({
        available,
        keys,
        workers: statuses ?? [],
        device: await savedDeviceWorker(),
        settings: await savedSettings(),
        resultCount: (await enrichMap()).size,
        hasSamples: await hasSampleEnrich(),
        lastRelease: lastRelease ?? _state?.lastRelease,
    });
};

/*
 * Confirm a worker on this device and start releasing keys to it. The caller
 * must have shown the user {@link workerFingerprint} of the key and had them
 * compare it with the fingerprint the worker prints.
 */
export const enableHomeAI = async (key: WorkerKey, fingerprint: string) => {
    if ((await workerFingerprint(key.publicKey)) != fingerprint) {
        throw new Error("Fingerprint does not match the worker key");
    }
    const statuses = (await fetchWorkerStatusIfAvailable()) ?? [];
    // Enabling again would rotate the token the worker already holds.
    if (!statuses.some((s) => s.workerPublicKey == key.publicKey)) {
        await enableWorker(key.publicKey);
    }
    await saveDeviceWorker(key, fingerprint);
    void homeAISync();
};

// Stop releasing keys from this device. The worker stays enabled.
export const stopHomeAIOnThisDevice = async () => {
    await clearDeviceWorker();
    await pullHomeAIState();
};

/*
 * Kill switch: revoke the worker's access for the whole account and drop the
 * keys it has not read yet.
 */
export const disableHomeAI = async () => {
    await disableWorkers();
    await clearDeviceWorker();
    await pullHomeAIState();
};

const userFiles = async (includeHidden: boolean): Promise<EnteFile[]> => {
    const userID = ensureLocalUser().id;
    const files = includeHidden
        ? await savedCollectionFiles()
        : await computeNormalCollectionFilesFromSaved(userID);
    return uniqueFilesByID(files.filter((f) => f.ownerID == userID));
};

/*
 * Release new keys and fetch new results. Called after each remote pull; a
 * call while one is running joins it.
 */
export const homeAISync = () =>
    (_syncPromise ??= sync().finally(() => {
        _syncPromise = undefined;
    }));

const sync = async () => {
    try {
        const workers = await fetchWorkerStatusIfAvailable();
        if (!workers) {
            await pullHomeAIState(workers);
            return;
        }
        const settings = await savedSettings();
        const lastRelease = await releaseKeysIfNeeded(
            await userFiles(settings.includeHidden),
            workers,
        );
        if (workers.length) {
            const files = await userFiles(true);
            if (await syncEnrich(new Map(files.map((f) => [f.id, f])))) {
                _enrich = undefined;
            }
        }
        await pullHomeAIState(workers, lastRelease);
    } catch (e) {
        log.error("Home AI sync failed", e);
    }
};

// The suggestions the review screen shows, from what this device has.
export const homeAISuggestions = async (): Promise<HomeAISuggestions> => {
    const userID = ensureLocalUser().id;
    const files = await userFiles(false);
    const collectionFiles = await computeNormalCollectionFilesFromSaved(userID);
    const albums = (await savedNormalCollections())
        .filter((c) => c.owner.id == userID && c.type == "album")
        .map((c) => ({
            id: c.id,
            name: c.name,
            fileIDs: new Set(
                collectionFiles
                    .filter((f) => f.collectionID == c.id)
                    .map((f) => f.id),
            ),
        }));
    return computeSuggestions(
        files,
        await enrichMap(),
        albums,
        await savedSettings(),
    );
};

export const loadHomeAISamples = async () => {
    await saveSampleEnrich(sampleEnrich(await userFiles(false)));
    _enrich = undefined;
    await pullHomeAIState();
};

export const clearHomeAISamples = async () => {
    await clearSampleEnrich();
    _enrich = undefined;
    await pullHomeAIState();
};

// Search bar suggestions from worker entities, devices and OCR text.
export const homeAISearchSuggestions = async (
    s: string,
    searchString: string,
): Promise<SearchSuggestion[]> => {
    if (!_state?.settings.search) return [];
    try {
        return entityMatches(s, searchString, await enrichMap()).map(
            ({ label, kind, fileIDs }) => ({
                type: "homeAI",
                kind,
                fileIDs,
                label,
            }),
        );
    } catch (e) {
        log.warn("Ignoring Home AI search failure", e);
        return [];
    }
};

export const logoutHomeAI = async () => {
    _state = undefined;
    _enrich = undefined;
    await clearEnrichDB();
    await clearDeviceWorker();
    await removeKV(settingsKey);
};
