# Home AI worker (P4, CPU-only)

A headless, read-only Ente client in Rust that runs on the home PC. Clients
seal each new file's fileKey to the worker's allowlisted X25519 key (P2,
[`fork/docs/worker-api.md`](../docs/worker-api.md)); the worker reads its
inbox, decrypts each file in memory, runs CPU analysis stages and merges the
result into the file's `mldata` under one key, `x_enrich`.

This is phase 4 of the plan: no GPU, no models. The GPU presets and model
stages exist in the config so the file format is stable, but this build
refuses them with a clear error.

It lives outside the upstream `rust/` workspace (its own `Cargo.toml` and
`Cargo.lock`) so the weekly upstream sync never touches it, and uses
`ente-core`, `ente-exif` and `ente-image` from `rust/crates/` by path.

## What one poll does

1. `GET /fork/worker/pairing` and open each user's sealed worker token.
2. Page through `GET /fork/worker-api/inbox`, `runtime.batch_size` items at
   a time. Each sealed fileKey is opened and moved into locked memory.
3. Batch-read the files' `mldata`. A file whose record lacks Ente's own
   current `face` and `clip` indexes is **deferred**: it stays in the inbox
   and is retried on later polls. Writing only to complete records means
   stock mobile and web never re-index and overwrite them (see
   "Clients and `x_enrich`" in the API doc).
4. For the rest: decrypt metadata, public magic metadata and the thumbnail;
   for still images under `runtime.max_original_mb`, also the original, for
   EXIF. Run the enabled stages.
5. Merge: replace only `x_enrich`, keep every other key as read (numbers
   included, byte for byte), gzip, encrypt with the fileKey, and
   `PUT /files/data` with the `lastUpdatedAt` it read. A 409 (someone wrote
   in between) defers the file instead of overwriting.
6. Wipe the batch's key slots and mark written files consumed. Sealed keys
   that can never be opened (an old worker keypair) are consumed too, with
   a warning.

A record already carrying the current `x_enrich` version is consumed without
work, so re-sent keys are cheap.

## CPU stages and the `x_enrich` shape

```json
{
  "version": 1,
  "client": "ente-fork-worker/0.1.0",
  "updatedAt": 1792760421156,
  "device": {"kind": "camera", "make": "Apple", "model": "iPhone 13"},
  "hashes": {"dhash": "16 hex digits", "phash": "16 hex digits"},
  "blur": {"laplacianVariance": 182.4, "isBlurry": false},
  "pii": {"method": "regex", "kinds": {"iban": 1}, "suggestHidden": true}
}
```

- **device**: camera make and model from EXIF; otherwise, if the pixel size
  matches a known screen, `{"kind": "screenshot", "candidates": [...]}`;
  else `{"kind": "unknown"}`. The size table is in `src/stages/device.rs`.
- **hashes**: difference hash and DCT hash of the thumbnail. A Hamming
  distance of about 10 or less means a near duplicate.
- **blur**: variance of the Laplacian of the thumbnail scaled to 512 px.
  `isBlurry` uses a starting threshold of 60, to be tuned on real photos.
- **pii**: regex with checksums on the title, caption and EXIF description:
  Luhn-checked cards, mod-97 IBANs, passport MRZ check digits, Indonesian
  NIK structure, email and phone. Only kinds and counts are stored, never
  values. `suggestHidden` is true for card, IBAN, MRZ or NIK; it is only a
  suggestion, never an action. OCR text joins these inputs once the OCR
  stage exists; GLiNER2 is the planned second check.

## Key custody

Required by the plan: keys never touch disk, swap or the pagefile.

- **Worker private key** is read once at start, from a systemd credential
  (`LoadCredentialEncrypted=`, sealed with the TPM where there is one) or
  from stdin. There is no plain-file option. It is moved into guarded memory
  and the buffer it came in is wiped.
- **Guarded memory** (`src/secure_mem/`): every secret lives in its own
  pages between no-access guard pages, locked in RAM, excluded from core
  dumps and forks, and no-access whenever no job is reading it. On Linux it
  uses `memfd_secret`, which also hides the pages from the kernel's direct
  map, and falls back to `mlock` if the kernel lacks it. On Windows it uses
  `VirtualLock`, and also encrypts idle pages with `CryptProtectMemory`. If
  locking fails, the worker does not start.
- **fileKeys** sit in one slab of `batch_size` slots, are lent to decryption
  only inside a closure, and are wiped when the batch ends, whatever its
  outcome.
- **Process**: not dumpable or ptrace-attachable by the same user
  (`PR_SET_DUMPABLE=0`), core dumps off, memlock limit raised to fit.
- `ente-worker doctor` reports what the host offers; missing protections
  are logged as warnings at every start.

Known gaps, all small windows:

- A key is copied onto the stack as `ente_core`'s `Key` for the length of
  one decrypt call (zeroized on drop); a freshly opened fileKey sits in a
  normal heap buffer for microseconds before it moves into the slab.
- The worker token is copied into the HTTP client's header (marked
  sensitive). It is revocable, unlike keys.
- Decrypted thumbnails, originals and pixels are in normal heap memory,
  zeroized on drop. They are photo content, not keys; the unit file's
  `MemorySwapMax=0` keeps the worker's whole cgroup out of swap anyway,
  without turning swap off for WSL2.
- WSL2: Windows can page the VM's memory (`vmmem`) to `pagefile.sys`
  whatever Linux `mlock` says. BitLocker covers that path; keep hibernation
  off (`powercfg /h off`). For the strongest custody, run the worker natively
  on Windows, or on Linux after migrating.
- Root or kernel access to the running worker host can still read keys.

## Setup (Linux or WSL2 with systemd)

```sh
cd fork/worker
cargo build --release
sudo install -m 0755 target/release/ente-worker /usr/local/bin/

# Keypair: the secret goes straight into an encrypted credential.
sudo mkdir -p /etc/credstore.encrypted /etc/ente-worker
ente-worker keygen | sudo systemd-creds encrypt --name=ente-worker-key - \
    /etc/credstore.encrypted/ente-worker-key
# stderr shows the public key and fingerprint.
```

Without a TPM (WSL2), `systemd-creds` falls back to a host key in
`/var/lib/systemd/credential.secret`; BitLocker protects that file.

Put the public key in museum's `fork.worker-keys` allowlist and in
`worker.toml` (`server.public_key`), starting from
[`worker.example.toml`](worker.example.toml). Then:

```sh
sudo install -m 0644 worker.example.toml /etc/ente-worker/worker.toml   # edit it
ente-worker check-config -c /etc/ente-worker/worker.toml
ente-worker doctor -c /etc/ente-worker/worker.toml
sudo install -m 0644 ente-worker.service /etc/systemd/system/
sudo systemctl enable --now ente-worker
journalctl -u ente-worker -f
```

To try it without systemd, set `identity.source = "stdin"` and pipe the key
in from a secret store, for example
`pass show ente/worker-key | ente-worker once -c worker.toml`.

## Configuration

One `worker.toml` picks the profile, stages, models, precision, batch size
and VRAM budget; every setting is in
[`worker.example.toml`](worker.example.toml). Unknown keys are an error.
`batch_size` is how many fileKeys are held in locked memory at once (and
the inbox page size); `concurrency` is how many files are analysed in
parallel. Defaults are the `cpu-only` preset: batch 1, FP32, no VRAM.

## Development

```sh
cd fork/worker
cargo fmt && cargo clippy --all-targets && cargo test
```

`tests/e2e.rs` runs a full poll against a mock museum. CI is
`.github/workflows/fork-worker.yml`, on Linux and Windows.

## Not in this phase

- GPU, models and the model stages (OCR, CLIP, faces, Laya, GLiNER2).
- The worker-side SQLCipher search index and the desktop IPC.
- A native Windows key source (DPAPI-NG or a TPM-backed CNG key); on
  Windows use `stdin` for now.
- The "no key bytes on disk" gate audit: run a full poll, then check with
  `blktrace` or `/proc/<pid>/io` on Linux and a pagefile scan on Windows.
- Sharee visibility: `x_enrich` (including the PII suggestion) is readable
  by anyone the file is shared with, like the rest of `mldata`. Fine for an
  own-library first release; the private PII flag is written by the client
  after you accept the suggestion (P3).
