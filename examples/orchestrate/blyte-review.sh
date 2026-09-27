#!/usr/bin/env bash
# Show what a dispatched run did, for review before merging.
#
#   blyte-review.sh <id>          commits, changed files, the tree's state and the end of blyte's log
#   blyte-review.sh <id> --diff   the same, then the full diff
#
# Run it from the repository's main checkout, after blyte-dispatch.sh. Merge with
# `git merge --ff-only wip/<id>` once satisfied, then `git worktree remove .wt/<id>` and
# `git branch -d wip/<id>`.
set -uo pipefail

[ $# -ge 1 ] || { sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
ID=$1
ROOT=$(git rev-parse --show-toplevel) || exit 2
WT=$ROOT/.wt/$ID
[ -d "$WT" ] || { echo "no worktree: $WT" >&2; exit 2; }
BASE=$(git -C "$WT" merge-base HEAD "$(git -C "$ROOT" rev-parse HEAD)")

echo "== commits"; git -C "$WT" log --oneline "$BASE"..HEAD
echo "== files";   git -C "$WT" diff --stat "$BASE"..HEAD
echo "== tree";    git -C "$WT" status --short; echo "(empty means clean)"
echo "== log";     tail -25 "$ROOT/.wt/$ID.log"
[ "${2:-}" = "--diff" ] && { echo "== diff"; git -C "$WT" diff "$BASE"..HEAD; }
exit 0
