// fork: the `x_enrich` key that the home AI worker writes into a file's
// `mldata`. The schema is documented in fork/docs/x-enrich.md; keep the two in
// sync.

import { z } from "zod";

/** The top-level `mldata` key the worker owns. */
const enrichMLDataKey = "x_enrich";

/** The private magicMetadata key where this client records review choices. */
export const reviewMagicMetadataKey = "x_review";

const Score = z.number().min(0).max(1);

// Each field falls back to undefined on its own, so one malformed field (say
// from a newer worker) does not hide everything else the worker found.
const lenient = <T extends z.ZodType>(schema: T) =>
    schema.optional().catch(undefined);

const EnrichEntity = z.object({
    kind: z.string(),
    name: z.string().min(1),
    ref: z.string().optional(),
    score: Score,
});

export type EnrichEntity = z.infer<typeof EnrichEntity>;

const XEnrich = z.object({
    v: z.literal(1),
    client: z.string(),
    at: z.number(),
    device: lenient(
        z.object({
            make: z.string().optional(),
            model: z.string().optional(),
            kind: z.string().optional(),
            score: Score.optional(),
        }),
    ),
    languages: lenient(z.array(z.string())),
    ocr: lenient(
        z.object({ text: z.string(), truncated: z.boolean().optional() }),
    ),
    entities: lenient(
        z.array(z.unknown()).transform((es) =>
            es.flatMap((e) => {
                const r = EnrichEntity.safeParse(e);
                return r.success ? [r.data] : [];
            }),
        ),
    ),
    album: lenient(
        z.object({
            name: z.string().min(1),
            group: z.string().optional(),
            score: Score,
        }),
    ),
    cleanup: lenient(
        z.object({
            score: Score,
            reasons: z.array(z.string()),
            duplicateOf: z.number().optional(),
        }),
    ),
    pii: lenient(z.object({ score: Score, kinds: z.array(z.string()) })),
});

export type XEnrich = z.infer<typeof XEnrich>;

/**
 * Return the worker's results from a decrypted `mldata` record, or undefined
 * if it has none or they are of an unknown version.
 */
export const enrichFromMLData = (
    raw: Record<string, unknown>,
): XEnrich | undefined => {
    const result = XEnrich.safeParse(raw[enrichMLDataKey]);
    return result.success ? result.data : undefined;
};

const ReviewChoices = z.object({
    album: z.enum(["accepted", "rejected"]).optional().catch(undefined),
    cleanup: z.enum(["keep"]).optional().catch(undefined),
    pii: z.enum(["hidden", "dismissed"]).optional().catch(undefined),
});

export type ReviewChoices = z.infer<typeof ReviewChoices>;

/** The review choices stored in a file's (decrypted) private magicMetadata. */
export const reviewChoicesFromMagicMetadata = (
    data: Record<string, unknown> | undefined,
): ReviewChoices => {
    const result = ReviewChoices.safeParse(data?.[reviewMagicMetadataKey]);
    return result.success ? result.data : {};
};
