# P0: run Ente locally and test it

How to run this fork's server and web app on your own PC, sign up, upload
photos, and reach it from your other devices. Part 1 is stock Ente (the P0
baseline). Part 2, optional, turns on the fork's Home AI pieces (P2 to P4).

Everything runs in Docker from this repository's source, using upstream's
`server/config/compose.yaml`, so the fork's server and web changes are
included. Upstream's prebuilt images (`ghcr.io/ente/server`, `quickstart.sh`)
are stock Ente and do **not** have the Home AI routes or UI.

What was checked and what wasn't is listed at the end.

## Part 1: stock baseline

### 1. Prerequisites

- **Windows 11 with WSL2**, a Linux distro (Ubuntu) inside it, and either
  - **Docker Desktop** with the WSL2 backend and "WSL integration" on for
    your distro (easiest), or
  - **Docker Engine inside the distro** (needs `systemd=true` in
    `/etc/wsl.conf`).
- Docker Compose **2.30 or newer** (`docker compose version`); the MinIO
  bucket setup uses `post_start`, which older versions ignore.
- `git` and `openssl` in the distro.
- RAM: museum, Postgres and MinIO need little, but building the web image
  (Next.js plus Rust wasm) wants about 6 to 8 GB. If the build is killed,
  raise `memory=` under `[wsl2]` in `%UserProfile%\.wslconfig`. Keep swap
  on.
- Clone into the Linux filesystem (`~/`), not `/mnt/c/...`: builds and
  Postgres are much slower and file permissions break on the Windows side.

```sh
cd ~
git clone https://github.com/KetanBasi/ente.git
cd ente/server/config
```

On Linux later, the same steps work unchanged.

### 2. Configure: `.env` and `museum.yaml`

Both files live in `server/config/` and are git-ignored. Upstream's
`example.env` and `example.yaml` are the templates; the values must match
between the two.

```sh
cd ~/ente/server/config
cp example.env .env
cp example.yaml museum.yaml
chmod 600 .env museum.yaml

# Fresh secrets (same recipe as upstream's quickstart.sh)
echo "postgres password: $(openssl rand -base64 21)"
echo "minio user:        minio-$(openssl rand -hex 4)"
echo "minio password:    $(openssl rand -base64 21)"
echo "key.encryption:    $(openssl rand -base64 32)"
echo "key.hash:          $(openssl rand -base64 64 | tr -d '\n')"
echo "jwt.secret:        $(openssl rand -base64 32 | tr '+/' '-_')"
```

Then edit, using those values:

- `.env`: `POSTGRES_PASSWORD`, `MINIO_ROOT_USER`, `MINIO_ROOT_PASSWORD`.
  Leave `ENTE_API_ORIGIN=http://localhost:8080` for now.
- `museum.yaml`:
  - `db.password` = the Postgres password;
  - each of the three buckets (`b2-eu-cen`, `wasabi-eu-central-2-v3`,
    `scw-eu-fr-v3`): `key` = MinIO user, `secret` = MinIO password. Keep
    `endpoint: localhost:3200` for now (step 7 changes it for other
    devices);
  - `key.encryption`, `key.hash`, `jwt.secret` = the generated values.
    **Do not keep the example's fixed keys.**

Keep a copy of `museum.yaml` and `.env` in your password manager. Without
them the data in the Docker volumes can't be used.

You don't need SMTP. With `smtp.host` empty, museum writes each email's
subject to its log instead of sending it, and that includes the sign-up
code.

### 3. Build and start

```sh
cd ~/ente/server/config
docker compose up -d --build
```

This builds two images from source, museum (a few minutes) and the web apps
(10 to 20 minutes the first time), and starts museum, Postgres, MinIO
(`pgsty/silo`, a MinIO build), a small `socat` helper and the web apps.

To start only the server, for example while the web image is still
building elsewhere, use
`docker compose up -d --build museum postgres minio socat`.

Check it:

```sh
docker compose ps                 # all Up; postgres (healthy)
curl http://localhost:8080/ping   # {"id":"","message":"pong"}
docker compose logs -f museum     # Ctrl-C to stop following
```

Ports: museum API `8080`, Photos web `3000`, public albums `3002`, MinIO
S3 API `3200`. The other `300x` ports are Ente's other web apps (Auth,
Cast, Locker ...), not needed for photos.

How the pieces connect: museum hands clients pre-signed URLs that point at
the bucket `endpoint` from `museum.yaml`. Clients upload and download
directly to MinIO with them, so that address must work **both** from inside
the museum container and from every client. The `socat` sidecar makes
`localhost:3200` inside museum's container point at MinIO; that is why
`localhost:3200` works on this PC.

### 4. Create your account

1. Open <http://localhost:3000> in a browser on the Windows side (WSL2
   forwards `localhost`).
2. "Don't have an account?", enter your email and a password.
3. The verification code is in museum's log:

   ```sh
   docker compose logs museum | grep -i "skipping sending email"
   ```

   The line reads
   `Skipping sending email to you@example.com: Verification code: 467570`.
4. Save the recovery key it shows. This account is separate from any
   ente.com account, even with the same email.

The first user is the admin. To allow more than 10 GB (the default free
plan), make it admin explicitly and lift the limit with the Ente CLI:

```sh
# your user id
docker compose exec postgres sh -c \
  'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "select user_id from users;"'
```

Add to `museum.yaml`:

```yaml
internal:
    admins:
        - <user_id>
```

then `docker compose up -d --force-recreate museum`. Install the Ente CLI
(`cli/` in this repo, or a release from upstream), put
`endpoint: {api: "http://localhost:8080"}` in `~/.ente/config.yaml`, and
run:

```sh
ente account add                         # log in as yourself
ente admin update-subscription -a you@example.com -u you@example.com --no-limit True
```

When you are the only user, also set `internal.disable-registration: true`
and recreate museum.

### 5. Upload test photos

Start with a small throwaway set (20 to 50 files: some JPEGs, a HEIC, a
screenshot, a short video, one with GPS EXIF). Keep the originals; this is a
test instance.

- **Web**: <http://localhost:3000>, Upload, pick files, choose an album
  (or Uncategorized), Start upload.
- **Desktop app**: install Ente Photos desktop (the stock release from
  ente.com, or the fork's unsigned build from Part 2). On the login screen,
  click the empty background 7 times, set the server endpoint to
  `http://localhost:8080`, then log in.

Google Takeout and the full library come after the checklist passes. The
desktop app's "Upload, Google Takeout" reads Takeout's JSON sidecars for
dates and locations.

### 6. Checklist

Tick each one before moving on:

- [ ] `curl localhost:8080/ping` returns `pong`.
- [ ] Sign-up worked with the code from the log; logout and login work.
- [ ] Uploaded photos show thumbnails and open full size on the web.
- [ ] The same photos appear on a second client (desktop or phone) logged
      in to the same account.
- [ ] A video plays.
- [ ] Create an album, add photos, and see it on the other client.
- [ ] Delete a photo, see it in Trash, restore it.
- [ ] Objects exist in MinIO:
      `docker compose exec minio sh -c 'mc alias set h0 http://localhost:3200 "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" >/dev/null && mc ls --recursive h0/b2-eu-cen | head'`
- [ ] Survives a restart: `docker compose down && docker compose up -d`,
      photos still there.
- [ ] Optional backup test: `docker compose exec -T postgres pg_dump -U pguser ente_db > ente_db.sql`
      and copy the `config_minio-data` volume.

### 7. Reach it from your other devices

Two things must be reachable from the phone or laptop: museum (`8080`) and
MinIO (`3200`), and the web app (`3000`) if you use it in a browser.

**a) Get the ports onto the network.**

- Docker Desktop: published ports are on Windows already; allow them in
  Windows Firewall when asked (private network only).
- Docker Engine inside WSL2: other devices can't see WSL2 ports by default.
  On Windows 11, put this in `%UserProfile%\.wslconfig` and run
  `wsl --shutdown`:

  ```ini
  [wsl2]
  networkingMode=mirrored
  ```

  WSL then shares the Windows IP. Allow the ports in Windows Firewall, and
  in the Hyper-V firewall if they're still blocked (PowerShell as admin):
  `New-NetFirewallHyperVRule -Name ente -DisplayName ente -Direction Inbound -VMCreatorId '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -Protocol TCP -LocalPorts 8080,3000,3200`.

**b) Pick a name every device can resolve.** Your devices span two subnets
behind two routers, so a plain LAN IP may only work from one side.

- **Tailscale (recommended).** Install it on Windows and on each device.
  Every device then reaches the PC at its MagicDNS name, e.g.
  `homepc.<tailnet>.ts.net`, from anywhere, with no port forwarding.
- **Fix the LAN instead.** Put the second router in access point (bridge)
  mode so everything shares one subnet, or add a static route between the
  two. Then use the PC's LAN IP, and give it a DHCP reservation.

**c) Use that name in the config.** With `HOST` = the Tailscale name or LAN
IP:

- `museum.yaml`: for all three buckets, `endpoint: HOST:3200`.
- `.env`: `ENTE_API_ORIGIN=http://HOST:8080`.
- With a Tailscale name, the museum container may not resolve MagicDNS.
  Then add it to the `museum` service in `compose.yaml`
  (a local edit; don't commit it):

  ```yaml
  extra_hosts:
      - "HOST:host-gateway"
  ```

  and drop the `socat` service, which only exists for `localhost`.

Then `docker compose up -d --force-recreate`. In each app, set the endpoint
to `http://HOST:8080` (mobile: tap the onboarding screen 7 times, Developer
settings). Web: <http://HOST:3000>.

Over plain `http` on a non-localhost address, some browser features need a
secure context. If the web app misbehaves there, use the desktop or mobile
app, or put HTTPS in front (`tailscale serve`, or Caddy; see upstream's
`docs/docs/self-hosting/administration/reverse-proxy.md`). Uploads from
another device that hang usually mean the bucket `endpoint` isn't
reachable from that device (museum logs `OBJECT_SIZE_FETCH_FAILED`).

### Day to day

```sh
cd ~/ente/server/config
docker compose up -d              # start
docker compose down               # stop (data stays in volumes)
git pull && docker compose up -d --build   # update to the fork's latest main
```

`docker compose down -v` deletes the volumes, which means **all photos and
the database**. Only for a fresh start.

## Part 2 (optional): Home AI

Turns on the fork's worker features on top of Part 1. Details:
[worker-api.md](worker-api.md) (server), [home-ai-client.md](home-ai-client.md)
(web and desktop UI), [../worker/README.md](../worker/README.md) (worker).

### 1. Make a worker keypair

Build the worker (Rust, CPU only) in WSL2:

```sh
cd ~/ente/fork/worker
cargo build --release
sudo install -m 0755 target/release/ente-worker /usr/local/bin/
```

With systemd (WSL2 with `systemd=true`, or Linux), seal the private key
straight into an encrypted credential; the public key and fingerprint are
printed on stderr:

```sh
sudo mkdir -p /etc/credstore.encrypted /etc/ente-worker
ente-worker keygen | sudo systemd-creds encrypt --name=ente-worker-key - \
    /etc/credstore.encrypted/ente-worker-key
```

Note the public key and the fingerprint.

### 2. Allowlist the key in museum

In `server/config/museum.yaml`:

```yaml
fork:
    worker-keys:
        - name: home-pc
          public-key: "<public key from keygen>"
```

`docker compose up -d --force-recreate museum`, then check:

```sh
docker compose logs museum | tail      # no config error
```

Museum refuses to start if the key isn't 32 bytes of base64.

### 3. Turn it on in the client

Home AI is only in the fork's web and desktop builds. The web app from
step 3 of Part 1 is built from this fork, so it has it. For desktop, use the
unsigned Windows build from the latest **Fork: Build** run on `main`
(Actions, the run, artifact for `windows-latest`, kept 7 days; re-run the
workflow if it expired).

1. Settings (the sidebar), **Home AI**, switch on "Analyse my library on
   Home AI".
2. Compare the fingerprint in the dialog with the one `keygen` printed.
   Confirm only if they match.
3. "Load sample results" fills the review screens with made-up data, to
   try the UI before the worker has done anything.

From then on, each sync seals new files' keys to the worker. Key release
only runs while a web or desktop client is open.

**Important:** the worker only writes to files that already have Ente's own
face and CLIP indexes, so the stock mobile app never overwrites its work.
On web and desktop those indexes are made only by the **desktop app**
(Settings, Machine learning, enable). Until a file is indexed there (or on
mobile), the worker leaves it in its inbox and retries.

### 4. Run the worker

```sh
sudo install -m 0644 ~/ente/fork/worker/worker.example.toml /etc/ente-worker/worker.toml
sudoedit /etc/ente-worker/worker.toml
#   [server] endpoint = "http://localhost:8080"   (or http://HOST:8080)
#            public_key = "<public key>"
ente-worker check-config -c /etc/ente-worker/worker.toml
ente-worker doctor -c /etc/ente-worker/worker.toml   # what key protection the host has
sudo install -m 0644 ~/ente/fork/worker/ente-worker.service /etc/systemd/system/
sudo systemctl enable --now ente-worker
journalctl -u ente-worker -f
```

Without systemd: set `[identity] source = "stdin"` and pipe the key from a
secret store, e.g. `pass show ente/worker-key | ente-worker once -c worker.toml`.
`once` processes the inbox and exits, which is handy for a first test; the
last log line is a summary like
`poll: Summary { users: 1, written: 0, deferred: 6, ... }`.

`deferred` with "no mldata yet" means the files have no face and CLIP index
yet (see the note in step 3). Only on a **throwaway test account**, you can
set `require_face_clip = false` under `[mldata]` to see the worker write
anyway; keep it `true` for your real library.

### Known gap: the web app doesn't show the worker's results yet

Found while testing this guide: the worker (P4) writes `x_enrich` with
`version`, `updatedAt`, `hashes`, `blur` and `pii.suggestHidden`, but the
web client (P3) only accepts the v1 shape in
[home-ai-client.md](home-ai-client.md) (`v`, `at`, `cleanup`, `pii.score`
...). So every real worker record is ignored and Home AI keeps showing "0
analysed photos". Pairing, key release, worker writes and the kill switch
all work. Until the two are aligned, use "Load sample results" to try the
review screens.

### 5. Checklist

- [ ] The Home AI confirm dialog shows `home-pc` with the same fingerprint
      `keygen` printed.
- [ ] After turning it on, Home AI shows "Sent, waiting for the worker"
      and the worker log shows `users: 1` and one line per file.
- [ ] Files indexed by the desktop app are `written`, not `deferred`.
      (Seeing them in Review suggestions waits on the known gap above.)
- [ ] Kill switch: switch off, "Turn off for my account"; the next
      `ente-worker once` shows `users: 0`.

## What was verified

Run in a Linux cloud sandbox (Docker 29, Compose 5.6) on 2026-10-31 against
fork `main` @ `7b8e95e`:

- Part 1: `.env` and `museum.yaml` from the examples with fresh secrets;
  museum, Postgres, MinIO and socat from `server/config/compose.yaml` with
  museum built from source; `/ping`; sign-up in the fork's web app with the
  code from the log; login; upload of 6 images (12 objects in MinIO, 6 rows
  in `files`); `docker compose down` and `up` with data intact.
- Part 2: museum starts with the `fork.worker-keys` allowlist and creates
  the fork tables; `ente-worker` builds, `keygen`, `check-config`,
  `doctor`; the web app's fingerprint matched `keygen`'s; turning Home AI on
  released 6 keys; `ente-worker once` (key on stdin) paired, deferred all 6
  (no ML index, as expected from a browser), wrote all 6 with
  `require_face_clip = false`; the kill switch dropped it to `users: 0`.

Not exactly as written: the sandbox can't reach Docker Hub or run builds
through its proxy unaided, so the base images came from a mirror, the
museum image was built from the same `Dockerfile` with proxy settings
added, and the web app was built with `npm run build:photos` and served by
nginx instead of the `ente-web` image.

Not verified (no hardware for it): WSL2 and Docker Desktop specifics,
Windows Firewall, mirrored networking, Tailscale and LAN access from other
devices, the desktop and mobile apps (including the desktop ML indexing the
worker waits for), the Ente CLI subscription change, systemd credentials and
the worker as a service.
