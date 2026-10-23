// fork: what happens when the user accepts or rejects a Home AI suggestion.
// Every action is the user's own; the worker never moves, hides or deletes
// anything. Choices are kept in the file's private magicMetadata (owner only,
// never seen by sharees) so a suggestion is not offered again on any device.

import { ensureLocalUser } from "ente-accounts/services/user";
import { authenticatedRequestHeaders, ensureOk } from "ente-base/http";
import { apiURL } from "ente-base/origins";
import type { EnteFile } from "ente-media/file";
import {
    createMagicMetadata,
    encryptMagicMetadata,
} from "ente-media/magic-metadata";
import {
    addToCollection,
    createAlbum,
    hideFiles,
    moveToTrash,
} from "../collection";
import { batched, computeNormalCollectionFilesFromSaved } from "../file";
import { savedCollections } from "../photos-fdb";
import { reviewMagicMetadataKey, type ReviewChoices } from "./enrich";
import type { AlbumSuggestion } from "./suggestions";
import { reviewChoices } from "./suggestions";

const saveReviewChoices = (files: EnteFile[], choices: ReviewChoices) =>
    batched(files, async (b) =>
        ensureOk(
            await fetch(await apiURL("/files/magic-metadata"), {
                method: "PUT",
                headers: await authenticatedRequestHeaders(),
                body: JSON.stringify({
                    metadataList: await Promise.all(
                        b.map(async (file) => ({
                            id: file.id,
                            magicMetadata: await encryptMagicMetadata(
                                createMagicMetadata(
                                    {
                                        ...file.magicMetadata?.data,
                                        [reviewMagicMetadataKey]: {
                                            ...reviewChoices(file),
                                            ...choices,
                                        },
                                    },
                                    file.magicMetadata?.version,
                                ),
                                file.key,
                            ),
                        })),
                    ),
                }),
            }),
        ),
    );

/**
 * Put the files into the suggested album: the user's existing album of that
 * name, else a new one.
 */
export const acceptAlbumSuggestion = async (
    suggestion: AlbumSuggestion,
    files: EnteFile[],
) => {
    const existing =
        suggestion.existingCollectionID !== undefined
            ? (await savedCollections()).find(
                  (c) => c.id == suggestion.existingCollectionID,
              )
            : undefined;
    const album = existing ?? (await createAlbum(suggestion.name));
    await addToCollection(album, files);
    await saveReviewChoices(files, { album: "accepted" });
};

export const rejectAlbumSuggestion = (files: EnteFile[]) =>
    saveReviewChoices(files, { album: "rejected" });

/** Move to trash. Trash keeps them for 30 days, as for any delete. */
export const trashCleanupSuggestions = (files: EnteFile[]) =>
    moveToTrash(files);

/** Keep: also the label a later learnable cleanup model trains on. */
export const keepCleanupSuggestions = (files: EnteFile[]) =>
    saveReviewChoices(files, { cleanup: "keep" });

/** Move to Hidden, which sits behind the device lock. */
export const hideSuggestedFiles = async (files: EnteFile[]) => {
    await saveReviewChoices(files, { pii: "hidden" });
    // Hide every copy, as the gallery does, so none stays in another album.
    const fileIDs = new Set(files.map((f) => f.id));
    const copies = (
        await computeNormalCollectionFilesFromSaved(ensureLocalUser().id)
    ).filter((f) => fileIDs.has(f.id));
    await hideFiles(copies);
};

export const dismissHideSuggestions = (files: EnteFile[]) =>
    saveReviewChoices(files, { pii: "dismissed" });
