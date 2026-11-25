#!/usr/bin/env bash
# Land a pull request: wait for its checks, rerun jobs GitHub cancelled before they started,
# ask for confirmation at the terminal, squash-merge, then remove the local worktree and branch.
#
# Usage: scripts/land.sh <pr number>
#
# The merge needs a person: the script reads the confirmation from the terminal and refuses to
# run without one, so it cannot merge from a pipe, a background job or a model's shell.
set -euo pipefail

MAX_RERUNS=3
POLL_SECONDS=30

die() {
    echo "land: $*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: scripts/land.sh <pr number>"
pr="$1"
case "$pr" in
'' | *[!0-9]*) die "the pull request must be a number, got: $pr" ;;
esac
command -v gh >/dev/null || die "gh is not installed"
command -v jq >/dev/null || die "jq is not installed"
[ -t 0 ] || die "no terminal: run this yourself, it asks before merging"

info="$(gh pr view "$pr" --json state,isDraft,title,headRefName,baseRefName,mergeable)" ||
    die "cannot read pull request #$pr"
state="$(jq -r .state <<<"$info")"
title="$(jq -r .title <<<"$info")"
head="$(jq -r .headRefName <<<"$info")"
base="$(jq -r .baseRefName <<<"$info")"
[ "$state" = "OPEN" ] || die "#$pr is $state, not open"
[ "$(jq -r .isDraft <<<"$info")" = "false" ] || die "#$pr is a draft"
[ "$(jq -r .mergeable <<<"$info")" != "CONFLICTING" ] || die "#$pr conflicts with $base"

echo "land: #$pr $title ($head -> $base)"

reruns=0
while :; do
    # gh exits 8 while checks are pending and 1 when one failed; the JSON is what is read.
    checks="$(gh pr checks "$pr" --json name,bucket,link || true)"
    jq -e 'type == "array" and length > 0' <<<"$checks" >/dev/null ||
        die "#$pr has no checks to wait for"

    failed="$(jq -r '.[] | select(.bucket == "fail") | .name' <<<"$checks")"
    [ -z "$failed" ] || die "checks failed on #$pr:
$failed"

    cancelled="$(jq -r '.[] | select(.bucket == "cancel") | .link' <<<"$checks")"
    if [ -n "$cancelled" ]; then
        [ "$reruns" -lt "$MAX_RERUNS" ] ||
            die "checks were cancelled $((MAX_RERUNS + 1)) times on #$pr; see $cancelled"
        reruns=$((reruns + 1))
        runs="$(sed -n 's|.*/actions/runs/\([0-9][0-9]*\).*|\1|p' <<<"$cancelled" | sort -u)"
        [ -n "$runs" ] || die "cannot find the run of a cancelled check: $cancelled"
        for run in $runs; do
            echo "land: rerunning cancelled jobs of run $run (attempt $reruns of $MAX_RERUNS)"
            gh run rerun "$run" --failed
        done
        sleep "$POLL_SECONDS"
        continue
    fi

    pending="$(jq -r '[.[] | select(.bucket == "pending")] | length' <<<"$checks")"
    [ "$pending" -gt 0 ] || break
    echo "land: waiting for $pending check(s)"
    sleep "$POLL_SECONDS"
done

echo "land: every check passed on #$pr"
printf 'land: type %s to squash-merge it into %s: ' "$pr" "$base"
read -r answer
[ "$answer" = "$pr" ] || die "not merged"

gh pr merge "$pr" --squash
git fetch --quiet origin "$base"

# The worktree is removed only when it has no changes; git refuses otherwise.
root="$(git rev-parse --show-toplevel)"
worktree="$(git -C "$root" worktree list --porcelain |
    awk -v ref="refs/heads/$head" '/^worktree /{w=substr($0,10)} $0=="branch " ref {print w}')"
if [ -n "$worktree" ]; then
    if [ "$worktree" = "$root" ]; then
        echo "land: $head is checked out here; switch branch and delete it yourself"
        exit 0
    fi
    git -C "$root" worktree remove "$worktree"
    echo "land: removed worktree $worktree"
fi
if git -C "$root" show-ref --verify --quiet "refs/heads/$head"; then
    # A squash merge leaves the branch unmerged in git's eyes, so the delete is forced.
    git -C "$root" branch -D "$head" >/dev/null
    echo "land: deleted local branch $head"
fi
echo "land: #$pr is merged; origin/$base is fetched"
