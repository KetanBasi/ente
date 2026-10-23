# Home AI client (P3)

The web and desktop photos app can send file keys to the home AI worker and
show what it finds. Server side: [worker-api.md](worker-api.md). The mobile app
is not changed yet (see Limits).

Code lives in new files; upstream files have small `fork:` hunks.

- `web/packages/new/photos/services/fork/`: worker API calls, `x_enrich`
  schema, key release, local result store, suggestions, review actions.
- `web/apps/photos/src/components/fork/`: settings drawer and review screen.
- Hunks in upstream files: sidebar entry (`Sidebar.tsx`), review host
  (`gallery.tsx`), sync hook (`pull.ts`), logout cleanup (`logout.ts`), and a
  `homeAI` search suggestion type (`search/types.ts`, `search/worker.ts`,
  `search/index.ts`, `SearchBar.tsx`).

## Turning it on

Sidebar, Home AI. If the server lists exactly one worker key the switch asks
for confirmation: the dialog shows the SHA-256 fingerprint the client computed
itself, and the user compares it with the one the worker prints. Only then does
the client call `POST /fork/worker/token` and start releasing keys. With
several allowlisted keys the user picks one first.

Each remote pull then runs `homeAISync`:

1. Rebuild which files the worker already has from the audit log
   (`GET /fork/worker/inbox`), so every device agrees.
2. Re-check the key is still on the allowlist and still has the confirmed
   fingerprint; otherwise stop and forget the worker on this device.
3. Seal `fileKey` of the user's own files not yet released with
   `crypto_box_seal` and upload them in batches (at most 5000 files per sync).
   Hidden files are skipped unless "Include hidden items" is on.
4. Fetch changed `mldata` (status diff), keep `x_enrich`, store it in the
   `fork-home-ai` IndexedDB.

The switch off asks: "Turn off for my account" calls `DELETE /fork/worker/token`
(kill switch); "Stop on this device" only stops this device's key release.

## `x_enrich` v1 (written by the worker, read here)

```json
{
  "v": 1, "client": "worker/0.1", "at": 1760000000000000,
  "device":   {"make": "Google", "model": "Pixel 8", "kind": "screenshot", "score": 0.9},
  "languages": ["en"],
  "ocr":      {"text": "...", "truncated": false},
  "entities": [{"kind": "repo|social|url|movie|show|anime|manga|person", "name": "ente-io/ente", "ref": "optional id", "score": 0.97}],
  "album":    {"name": "Open source", "group": "optional cluster id", "score": 0.8},
  "cleanup":  {"score": 0.9, "reasons": ["duplicate|blur|accidental"], "duplicateOf": 123},
  "pii":      {"score": 0.7, "kinds": ["card_number|iban|passport|national_id|id_document|credential"]}
}
```

Every field except `v`, `client` and `at` is optional. A malformed field is
dropped on its own. Unknown `v` is ignored. Scores are 0 to 1.

## Review, search

Review (Home AI, Review suggestions) has three tabs. Nothing happens until the
user selects photos and confirms.

| Tab | Accept | Reject |
| --- | --- | --- |
| Albums | create the album (or add to the user's album of that name) | "Not this album" |
| Cleanup | move to trash (30 days) | "Keep" |
| Hide | move to Hidden | "Not sensitive" |

Reject and accept choices are written to the file's private magicMetadata under
`x_review` (owner only) so a suggestion is not shown again on any device. The
worker token cannot write this; the plan's PII flag is therefore set by the
client, as noted in worker-api.md.

Search shows worker entities, device names and OCR text (3+ characters) after
Ente's own results; it can be turned off in the Home AI settings.

"Load sample results" fills the local store with made-up results for roughly a
third of the files, to try the screens before the worker exists. They are
marked, never uploaded, and never replace real results.

## Limits

- Mobile is unchanged. Its `putFileData` still sends no `lastUpdatedAt`, so it
  can overwrite a worker write that lands between its fetch and upload. The
  worker only adds `x_enrich` to records that already have face and clip, which
  mobile does not rewrite. A Dart fix was left out because this environment has
  no Flutter toolchain to build or test it.
- Key release runs only while a web or desktop client is open and syncing.
- Strings are English only; the fork adds none to upstream's translations.
- Thresholds are per device (stored in the local KV).
