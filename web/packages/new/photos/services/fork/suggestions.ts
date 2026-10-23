// fork: turn the worker's `x_enrich` results into the suggestions the Home AI
// review screen and search show. Pure functions, so they can be tested
// without a server.

import type { EnteFile } from "ente-media/file";
import {
    reviewChoicesFromMagicMetadata,
    type ReviewChoices,
    type XEnrich,
} from "./enrich";

/** Per-device thresholds and switches, set in the Home AI settings. */
export interface HomeAISettings {
    /** Release keys of files in hidden albums too. */
    includeHidden: boolean;
    /** Suggest an album only when the worker is at least this sure. */
    minAlbumScore: number;
    minCleanupScore: number;
    minPIIScore: number;
    /** Offer worker entities and text in the search bar. */
    search: boolean;
}

export const defaultHomeAISettings: HomeAISettings = {
    includeHidden: false,
    minAlbumScore: 0.6,
    minCleanupScore: 0.7,
    minPIIScore: 0.5,
    search: true,
};

export interface AlbumSuggestion {
    /** Stable within one computation: the worker's group, else the name. */
    id: string;
    name: string;
    files: EnteFile[];
    /** The user's own album of the same name, if one exists. */
    existingCollectionID: number | undefined;
}

export interface ScoredFile {
    file: EnteFile;
    score: number;
    /** Worker reasons (cleanup) or PII kinds. */
    labels: string[];
}

export interface HomeAISuggestions {
    albums: AlbumSuggestion[];
    cleanup: ScoredFile[];
    hide: ScoredFile[];
}

/** An album must collect at least this many files to be suggested. */
const minAlbumSize = 2;

export const reviewChoices = (file: EnteFile): ReviewChoices =>
    reviewChoicesFromMagicMetadata(
        file.magicMetadata?.data as Record<string, unknown> | undefined,
    );

/**
 * @param files The user's own files, unique by ID, without hidden files.
 * @param enrichByFileID The worker results we have.
 * @param albums The user's own albums, to find ones that already exist and
 * to skip files already in them.
 */
export const computeSuggestions = (
    files: EnteFile[],
    enrichByFileID: Map<number, XEnrich>,
    albums: { id: number; name: string; fileIDs: Set<number> }[],
    settings: HomeAISettings,
): HomeAISuggestions => {
    const albumByName = new Map(
        albums.map((a) => [a.name.trim().toLowerCase(), a]),
    );

    const albumGroups = new Map<string, AlbumSuggestion>();
    const cleanup: ScoredFile[] = [];
    const hide: ScoredFile[] = [];

    for (const file of files) {
        const enrich = enrichByFileID.get(file.id);
        if (!enrich) continue;
        const choices = reviewChoices(file);

        const album = enrich.album;
        if (album && !choices.album && album.score >= settings.minAlbumScore) {
            const existing = albumByName.get(album.name.trim().toLowerCase());
            if (!existing?.fileIDs.has(file.id)) {
                const id = album.group ?? album.name;
                let group = albumGroups.get(id);
                if (!group) {
                    group = {
                        id,
                        name: album.name,
                        files: [],
                        existingCollectionID: existing?.id,
                    };
                    albumGroups.set(id, group);
                }
                group.files.push(file);
            }
        }

        if (
            enrich.cleanup &&
            !choices.cleanup &&
            enrich.cleanup.score >= settings.minCleanupScore
        ) {
            cleanup.push({
                file,
                score: enrich.cleanup.score,
                labels: enrich.cleanup.reasons,
            });
        }

        if (
            enrich.pii &&
            !choices.pii &&
            enrich.pii.score >= settings.minPIIScore
        ) {
            hide.push({
                file,
                score: enrich.pii.score,
                labels: enrich.pii.kinds,
            });
        }
    }

    const byScore = (a: ScoredFile, b: ScoredFile) => b.score - a.score;
    return {
        albums: [...albumGroups.values()]
            .filter((g) => g.files.length >= minAlbumSize)
            .sort(
                (a, b) =>
                    b.files.length - a.files.length ||
                    a.name.localeCompare(b.name),
            ),
        cleanup: cleanup.sort(byScore),
        hide: hide.sort(byScore),
    };
};

export interface EntityMatch {
    /** What the search bar shows, e.g. "ente-io/ente". */
    label: string;
    /** The worker's entity kind, or "text" for a match in the OCR text. */
    kind: string;
    fileIDs: number[];
}

/** Matches are capped so a short query does not flood the search bar. */
const maxEntityMatches = 8;

/**
 * Find worker entities, device names and OCR text that match a search
 * string. Entities come first, then devices, then one "text" match.
 *
 * @param s The search string, already trimmed and lowercased.
 * @param searchString The search string as the user typed it.
 */
export const entityMatches = (
    s: string,
    searchString: string,
    enrichByFileID: Map<number, XEnrich>,
): EntityMatch[] => {
    if (s.length < 2) return [];

    const entities = new Map<string, EntityMatch>();
    const devices = new Map<string, EntityMatch>();
    const textFileIDs: number[] = [];

    const add = (
        into: Map<string, EntityMatch>,
        label: string,
        kind: string,
        fileID: number,
    ) => {
        const key = `${kind}\u0000${label.toLowerCase()}`;
        const match = into.get(key);
        if (!match) into.set(key, { label, kind, fileIDs: [fileID] });
        else if (match.fileIDs.at(-1) != fileID) match.fileIDs.push(fileID);
    };

    for (const [fileID, enrich] of enrichByFileID) {
        for (const e of enrich.entities ?? []) {
            if (e.name.toLowerCase().includes(s)) {
                add(entities, e.name, e.kind, fileID);
            }
        }
        const device = [enrich.device?.make, enrich.device?.model]
            .filter(Boolean)
            .join(" ");
        if (device.toLowerCase().includes(s)) {
            add(devices, device, "device", fileID);
        }
        if (s.length >= 3 && enrich.ocr?.text.toLowerCase().includes(s)) {
            textFileIDs.push(fileID);
        }
    }

    const bySize = (a: EntityMatch, b: EntityMatch) =>
        b.fileIDs.length - a.fileIDs.length || a.label.localeCompare(b.label);
    const result = [
        ...[...entities.values()].sort(bySize),
        ...[...devices.values()].sort(bySize),
    ].slice(0, maxEntityMatches - 1);
    if (textFileIDs.length) {
        result.push({
            label: searchString,
            kind: "text",
            fileIDs: textFileIDs,
        });
    }
    return result;
};
