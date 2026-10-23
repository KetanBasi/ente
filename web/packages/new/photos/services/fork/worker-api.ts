// fork: owner-side calls to the home AI worker routes that the fork's museum
// patch adds. See fork/docs/worker-api.md.

import { authenticatedRequestHeaders, ensureOk } from "ente-base/http";
import { apiURL } from "ente-base/origins";
import { nullToUndefined } from "ente-utils/transform";
import { z } from "zod";

const WorkerKey = z.object({
    name: z.string(),
    publicKey: z.string(),
    // Server supplied, for display only. Clients compute their own.
    fingerprint: z.string().nullish().transform(nullToUndefined),
});

export type WorkerKey = z.infer<typeof WorkerKey>;

const WorkerStatus = z.object({
    workerPublicKey: z.string(),
    createdAt: z.number(),
    lastUsedAt: z.number().nullish().transform(nullToUndefined),
    pendingCount: z.number(),
    consumedCount: z.number(),
    tokenID: z.string(),
    workerName: z.string().nullish().transform(nullToUndefined),
});

export type WorkerStatus = z.infer<typeof WorkerStatus>;

const AuditItem = z.object({
    id: z.number(),
    fileID: z.number(),
    workerPublicKey: z.string(),
    createdAt: z.number(),
    consumedAt: z.number().nullish().transform(nullToUndefined),
});

export type AuditItem = z.infer<typeof AuditItem>;

/** One fileKey sealed to the worker with crypto_box_seal, base64. */
export interface SealedFileKey {
    fileID: number;
    sealedFileKey: string;
}

/** Museum caps one inbox upload at this many files. */
export const maxInboxBatch = 1000;

/**
 * Museums without the fork patch answer 404 here, which callers treat as
 * "Home AI is not available on this server".
 */
export const fetchWorkerKeys = async (): Promise<WorkerKey[]> => {
    const res = await fetch(await apiURL("/fork/worker/keys"), {
        headers: await authenticatedRequestHeaders(),
    });
    ensureOk(res);
    return (
        z.object({ keys: z.array(WorkerKey).nullish() }).parse(await res.json())
            .keys ?? []
    );
};

export const fetchWorkerStatus = async (): Promise<WorkerStatus[]> => {
    const res = await fetch(await apiURL("/fork/worker/status"), {
        headers: await authenticatedRequestHeaders(),
    });
    ensureOk(res);
    return (
        z
            .object({ workers: z.array(WorkerStatus).nullish() })
            .parse(await res.json()).workers ?? []
    );
};

/** Enable the worker for this account, or rotate its token. */
export const enableWorker = async (workerPublicKey: string) => {
    const res = await fetch(await apiURL("/fork/worker/token"), {
        method: "POST",
        headers: await authenticatedRequestHeaders(),
        body: JSON.stringify({ workerPublicKey }),
    });
    ensureOk(res);
    return WorkerStatus.parse(await res.json());
};

/**
 * Kill switch: revokes every worker token of this account and drops the
 * sealed keys the worker has not read yet.
 */
export const disableWorkers = async () =>
    ensureOk(
        await fetch(await apiURL("/fork/worker/token"), {
            method: "DELETE",
            headers: await authenticatedRequestHeaders(),
        }),
    );

/** Upload at most {@link maxInboxBatch} sealed keys. */
export const releaseKeys = async (
    workerPublicKey: string,
    keys: SealedFileKey[],
) => {
    const res = await fetch(await apiURL("/fork/worker/inbox"), {
        method: "POST",
        headers: await authenticatedRequestHeaders(),
        body: JSON.stringify({ workerPublicKey, keys }),
    });
    ensureOk(res);
    return z.object({ added: z.number() }).parse(await res.json()).added;
};

/** One page of the release log, oldest first. It never carries key bytes. */
export const fetchAuditLog = async (
    sinceID: number,
    limit = 1000,
): Promise<AuditItem[]> => {
    const params = new URLSearchParams({
        sinceID: sinceID.toString(),
        limit: limit.toString(),
    });
    const res = await fetch(
        `${await apiURL("/fork/worker/inbox")}?${params.toString()}`,
        { headers: await authenticatedRequestHeaders() },
    );
    ensureOk(res);
    return (
        z
            .object({ items: z.array(AuditItem).nullish() })
            .parse(await res.json()).items ?? []
    );
};
