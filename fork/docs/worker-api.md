# Home AI worker: server API (P2)

The fork's only server change. It lets one allowlisted worker (the home PC)
read the files a user sends it keys for, and write their `mldata`. The
worker never gets a collection key or the master key, and its token cannot
edit, share or delete anything.

Code: `server/pkg/controller/forkworker/`, `server/pkg/repo/forkworker/`,
`server/pkg/api/fork_worker.go`, `server/pkg/middleware/fork_worker_auth.go`,
`server/pkg/controller/filedata/fork_worker.go`, `server/migrations/fork/`.
The upstream hunks are two `fork:` blocks in `server/cmd/museum/main.go`.

## Configure the allowlist

In `museum.yaml`:

```yaml
fork:
    worker-keys:
        - name: home-pc
          public-key: "<standard base64 of the worker's X25519 public key>"
```

Museum refuses to start if a key is not 32 bytes of base64 or a name or key
repeats. No entries means the feature is off. Removing a key from the list
kills its tokens at once; rotating the worker keypair also makes any unread
sealed keys useless.

## Flow

1. The client lists `GET /fork/worker/keys`, shows the fingerprint (hex
   SHA-256 of the raw public key, which the client computes itself) and asks
   the user to confirm it once.
2. The client calls `POST /fork/worker/token`. Museum makes a random token,
   stores only its SHA-256, and stores the token sealed to the worker key
   with `crypto_box_seal`. Nobody but the worker can open it, the user
   included. Calling it again rotates the token.
3. The worker polls `GET /fork/worker/pairing?workerPublicKey=...` (public)
   and opens its sealed tokens. It never needs to store a token.
4. During background sync, clients seal each new file's fileKey to the worker
   key and upload batches to `POST /fork/worker/inbox`. Only the file's
   owner can do this, and only while the worker is enabled.
5. The worker reads `GET /fork/worker-api/inbox`, fetches and decrypts the
   files, writes `mldata`, and marks rows read with
   `POST /fork/worker-api/inbox/consume`.
6. Kill switch: `DELETE /fork/worker/token` revokes every worker token of the
   user and drops sealed keys the worker has not read yet. Read rows stay as
   the audit log (`GET /fork/worker/inbox`), which never returns key bytes.

## Routes

Owner (user session):

| Method and path | What it does |
| --- | --- |
| `GET /fork/worker/keys` | Allowlist with fingerprints |
| `GET /fork/worker/status` | Enabled workers, last use, pending and read counts |
| `POST /fork/worker/token` | `{workerPublicKey}`: enable or rotate |
| `DELETE /fork/worker/token` | Disable all workers (kill switch) |
| `POST /fork/worker/inbox` | `{workerPublicKey, keys: [{fileID, sealedFileKey}]}`, up to 1000; base64 of the 80 byte `crypto_box_seal` output |
| `GET /fork/worker/inbox?sinceID&limit` | Audit log |

Public: `GET /fork/worker/pairing?workerPublicKey=...` returns
`{pairings: [{userID, sealedToken, createdAt}]}`.

Worker (`X-Worker-Access-Token` header). Every file route first checks that
the token's user released that file to this worker:

| Method and path | What it does |
| --- | --- |
| `GET /fork/worker-api/inbox?sinceID&limit` | Unread sealed fileKeys, oldest first, up to 1000 |
| `POST /fork/worker-api/inbox/consume` | `{fileIDs}`: mark read |
| `GET /fork/worker-api/file-info/:fileID` | Decryption headers, encrypted metadata, public magic metadata, info |
| `GET /fork/worker-api/files/download/:fileID`, `.../files/preview/:fileID` | Redirect to the original or thumbnail |
| `GET /fork/worker-api/files/download/v3/:fileID`, `.../files/thumbnail/v3/:fileID` | Same, as `{url}` |
| `GET /fork/worker-api/files/data?fileID` | That file's `mldata` (204 if none) |
| `POST /fork/worker-api/files/data/fetch` | `{fileIDs}`: batch `mldata` read, up to 200 |
| `PUT /fork/worker-api/files/data` | Write `mldata`; `lastUpdatedAt` is required (0 if none yet) |

`server/pkg/api/fork_worker_test.go` pins this list: a new worker route
fails the test until it is added there on purpose.

## What the server can and cannot enforce

`mldata` is one record encrypted with the fileKey, so museum cannot see which
keys inside it the worker changed. It enforces `mldata` only, released files
only, and a mandatory `lastUpdatedAt`, so the worker can only replace the
exact record it read. Keeping Ente's `face` and `clip` keys intact and
writing only `x_enrich` is the worker's job (P4): read, merge, write.

## Clients and `x_enrich` (checked in code, not on a device)

- Web (`web/packages/new/photos/services/ml/`) parses `face` and `clip` as
  optional, keeps unknown keys when it writes, and sends `lastUpdatedAt`. A
  record with only `x_enrich` is re-indexed and the key survives.
- Mobile (`mobile/apps/photos/lib/utils/ml_util.dart`, `ml_service.dart`)
  re-indexes a record without compatible `face`/`clip`, starting from the
  remote record, so `x_enrich` survives too. But mobile's `putFileData` does
  not send `lastUpdatedAt`, so if the worker writes between mobile's fetch
  and its upload, mobile overwrites the worker's write.
- So the worker should add `x_enrich` only to records that already have
  current `face` and `clip` (then mobile and web never rewrite them), and
  treat `x_enrich` as rebuildable if it disappears. Sending `lastUpdatedAt`
  from mobile is a small P3 client fix.

## Known limits

- The pairing route is public and lists user IDs that enabled a worker key.
  Fine for a home server; a challenge-response would hide it.
- The plan stores the PII flag in private magicMetadata, which only the owner
  can write. The worker token cannot write it, so in P3/P4 the client should
  read the worker's suggestion and write the flag after the user accepts.
- Trashing a released file does not drop its inbox row; the row goes only
  when the file row is deleted. Untested whether a trashed file can still be
  downloaded through the worker routes.
