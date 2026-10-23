# Ente fork

This repository is a personal fork of [ente-io/ente](https://github.com/ente-io/ente),
used as a self-hosted Google Photos replacement with an AI worker on a home
PC. Everything specific to the fork is listed here; everything else is stock
Ente and should stay that way.

## Branches

| Branch               | What it is                                                                             |
| -------------------- | -------------------------------------------------------------------------------------- |
| `main`               | Upstream `main` plus the fork's changes. Default branch. Only changes through a PR.    |
| `fork/sync-upstream` | Written by the weekly sync job. Force-pushed by the bot; never commit to it.           |
| `fork/<phase>-<topic>` | Feature work, e.g. `fork/p2-worker-token`. Branch from `main`, PR back into `main`.  |

Upstream release branches (`release/*`) and tags are not mirrored.

## Keeping fork code separate

Upstream moves fast, so the fork stays mergeable by touching as few upstream
files as possible:

- Prefer new files and directories over edits to existing ones. Fork-only
  CI lives in `.github/workflows/fork-*.yml`, and fork tooling and docs live
  in `fork/`.
- When an upstream file has to change (a route registration, a menu entry),
  keep the hunk small and mark it with a `fork:` comment so it is easy to
  find during a conflict.
- Never reformat or reorder upstream code in a fork PR.
- Do not edit upstream workflows. Upstream-only ones are disabled through the
  API instead (see below).

## CI

- **`Fork: Build`** (`fork-build.yml`) runs on every PR and on pushes to
  `main`. It builds from source, unsigned:
  - server: `museum` binary, `go vet`, the Postgres-backed test suite, and
    the Docker image (not pushed);
  - web: the photos app and the web test suite;
  - desktop: the photos app on Linux and Windows, uploaded as an unpacked
    build artifact for 7 days;
  - Android: a debug APK of the photos app (`independent` flavor, arm64),
    uploaded as an artifact for 7 days.
- **`CI`** (upstream `ci.yml`) also runs on every PR and picks lint and test
  jobs by changed path, as it does upstream.

## Upstream sync

`fork-upstream-sync.yml` runs every Monday at 03:00 UTC and on demand:

1. It merges upstream `main` into `fork/sync-upstream` and opens or updates
   a PR into `main`. Merge that PR with a **merge commit**, not squash or
   rebase, so the next sync only brings in new commits.
2. It disables upstream workflows that need Ente's secrets or deploy
   Ente's services: anything triggered by `push`, `schedule`,
   `pull_request_target` or `release`, except `ci.yml` and
   `change-approval.yml`. Lint and test workflows stay enabled. A new
   upstream workflow is caught on the next run.

The sync merges instead of rebasing: a rebase would rewrite `main` and need a
force-push every week, and would break open feature branches. With fork code
in separate files, most weeks merge cleanly.

When the job fails with a merge conflict it prints the conflicting paths and
pushes nothing. Resolve it by hand:

```sh
git remote add upstream https://github.com/ente-io/ente.git   # once
git fetch upstream main
git checkout -B fork/sync-upstream origin/main
git merge upstream/main          # fix conflicts, keep the fork: hunks
git push --force origin fork/sync-upstream
```

then open the PR into `main`. `fork/scripts/sync-upstream.sh` does the same
when the merge is clean and `gh` is signed in.

## One-time GitHub setup

1. **Enable Actions** on this fork: Actions tab, "I understand my workflows,
   go ahead and enable them". GitHub turns Actions off for new forks.
2. **Enable the weekly sync**: GitHub also disables scheduled workflows in
   forks. Open "Fork: Upstream sync" in the Actions tab, enable it, and press
   "Run workflow" once. The first run disables the upstream-only workflows.
3. **Optional, recommended: `FORK_SYNC_TOKEN`.** The default `GITHUB_TOKEN`
   cannot push commits that change `.github/workflows`, which upstream does
   often, and its PRs don't start CI (the job starts `Fork: Build` itself as
   a workaround, but not upstream `CI`). Create a fine-grained personal
   access token for this repository only with **Contents**, **Pull
   requests** and **Workflows** set to read and write, and save it as the
   repository secret `FORK_SYNC_TOKEN`.
4. **Optional: protect `main`.** Require a PR and the `Fork: Build` checks
   before merging.

## State at fork setup (P1)

- 2026-10-23: fork `main` equals upstream `main` at `f5ad70a`. The plan was
  verified against `5d47314`; the 28 upstream commits since then touch none
  of `server/`, `web/packages/`, `architecture/` or the `core`, `photos` and
  `ml` Rust crates, so the plan's verified facts still hold.
- Building, signing and deploying for real use, and a stock self-hosted
  baseline (P0), are not part of this setup.

## License

Ente is AGPL-3.0, and so is this fork. Anyone other than the owner who uses
a deployment of it over a network must be offered the modified source.
