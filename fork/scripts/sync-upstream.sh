#!/usr/bin/env bash
# Merges upstream ente-io/ente main into a sync branch and opens (or updates)
# a pull request into the fork's main. On a merge conflict nothing is pushed:
# the conflicting paths are printed and the script fails, so the weekly run
# shows red and the merge is done by hand (see fork/README.md).
#
# Runs in .github/workflows/fork-upstream-sync.yml; also works locally from a
# clone of the fork with gh signed in.
#
# Env: UPSTREAM_URL (default ente-io/ente), UPSTREAM_BRANCH (default main),
#      SYNC_BRANCH (default fork/sync-upstream), BASE_BRANCH (default main),
#      DISPATCH_CI=1 to start fork-build.yml on the sync branch, for tokens
#      whose pushes don't trigger workflows (the default GITHUB_TOKEN).

set -euo pipefail

upstream_url="${UPSTREAM_URL:-https://github.com/ente-io/ente.git}"
upstream_branch="${UPSTREAM_BRANCH:-main}"
sync_branch="${SYNC_BRANCH:-fork/sync-upstream}"
base_branch="${BASE_BRANCH:-main}"

if git remote get-url upstream >/dev/null 2>&1; then
    git remote set-url upstream "$upstream_url"
else
    git remote add upstream "$upstream_url"
fi

git fetch --no-tags --filter=blob:none origin "$base_branch"
git fetch --no-tags --filter=blob:none upstream "$upstream_branch"

base="origin/$base_branch"
theirs="upstream/$upstream_branch"

if git merge-base --is-ancestor "$theirs" "$base"; then
    echo "$base_branch already contains $theirs ($(git rev-parse --short "$theirs"))."
    exit 0
fi

behind="$(git rev-list --count "$base..$theirs")"
theirs_sha="$(git rev-parse --short "$theirs")"
echo "$base_branch is $behind commits behind $theirs ($theirs_sha)."

git checkout -B "$sync_branch" "$base"
if ! git merge --no-ff --no-edit -m "Merge upstream ente-io/ente $upstream_branch @ $theirs_sha" "$theirs"; then
    echo "::error::Merge conflict with upstream. Conflicting paths:"
    git diff --name-only --diff-filter=U | sed 's/^/  /'
    git merge --abort
    exit 1
fi

if ! git push --force origin "$sync_branch"; then
    echo "::error::Push failed. If upstream changed .github/workflows, the default"
    echo "::error::GITHUB_TOKEN may not push it: add a FORK_SYNC_TOKEN secret (see fork/README.md)."
    exit 1
fi

title="Sync upstream ente-io/ente ($behind commits, $theirs_sha)"
body="Merges upstream \`$upstream_branch\` @ \`$theirs_sha\` into \`$base_branch\`.

Opened by \`fork/scripts/sync-upstream.sh\`. Merge with a merge commit (not squash or rebase) so the next sync only brings in new commits."

existing="$(gh pr list --head "$sync_branch" --base "$base_branch" --state open --json number --jq '.[0].number // empty')"
if [ -n "$existing" ]; then
    gh pr edit "$existing" --title "$title" --body "$body"
    echo "Updated PR #$existing."
else
    gh pr create --head "$sync_branch" --base "$base_branch" --title "$title" --body "$body"
fi

if [ "${DISPATCH_CI:-0}" = 1 ]; then
    gh workflow run fork-build.yml --ref "$sync_branch"
    echo "Started fork-build.yml on $sync_branch."
fi
