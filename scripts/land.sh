#!/usr/bin/env bash
# Land a pull request from this machine: run the checks here instead of on GitHub's runners, ask
# for confirmation at the terminal, squash-merge, then remove the local branch and worktree.
#
# Usage: scripts/land.sh <pr number>            run the checks, ask, merge
#        scripts/land.sh --yes <pr number>      run the checks, merge without asking
#        scripts/land.sh --check <pr number>    run the checks only
#
# Run it from a checkout of the pull request's branch, with nothing uncommitted and the branch
# pushed: the checks run on exactly the commit that will be merged.
#
# Without --yes the script asks at the terminal before merging and refuses to merge when there
# is none. --yes is for a shell with no terminal, such as an assistant asked to land the pull
# request: naming the number with --yes is the confirmation.
set -euo pipefail

TITLE_TYPES='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'

die() {
    echo "land: $*" >&2
    exit 1
}

check_only=false
assume_yes=false
case "${1:-}" in
--check)
    check_only=true
    shift
    ;;
--yes)
    assume_yes=true
    shift
    ;;
esac
[ $# -eq 1 ] || die "usage: scripts/land.sh [--check | --yes] <pr number>"
pr="$1"
case "$pr" in
'' | *[!0-9]*) die "the pull request must be a number, got: $pr" ;;
esac
for tool in gh jq cargo cargo-deny python3 mkdocs; do
    command -v "$tool" >/dev/null || die "$tool is not installed; a check needs it"
done
if ! $check_only && ! $assume_yes; then
    [ -t 0 ] || die "no terminal to ask at: pass --yes to merge without asking, or --check for the checks only"
fi

info="$(gh pr view "$pr" --json state,isDraft,title,headRefName,headRefOid,baseRefName,mergeable)" ||
    die "cannot read pull request #$pr"
state="$(jq -r .state <<<"$info")"
title="$(jq -r .title <<<"$info")"
head="$(jq -r .headRefName <<<"$info")"
head_sha="$(jq -r .headRefOid <<<"$info")"
base="$(jq -r .baseRefName <<<"$info")"
[ "$state" = "OPEN" ] || die "#$pr is $state, not open"
[ "$(jq -r .isDraft <<<"$info")" = "false" ] || die "#$pr is a draft"
[ "$(jq -r .mergeable <<<"$info")" != "CONFLICTING" ] || die "#$pr conflicts with $base"

here="$(git rev-parse --show-toplevel)"
cd "$here"
[ "$(git rev-parse HEAD)" = "$head_sha" ] ||
    die "this checkout is at $(git rev-parse --short HEAD), but #$pr is at ${head_sha:0:8}: check out $head here and push it"
[ -z "$(git status --porcelain)" ] || die "this checkout has uncommitted changes; the checks would not test what is merged"

echo "land: #$pr $title ($head -> $base) at ${head_sha:0:8}"

step() {
    local name="$1"
    shift
    echo "land: == $name"
    "$@" || die "check failed: $name"
}

title_check() {
    # The same types the pull request title check on GitHub allowed; the squash commit takes the title.
    grep -Eq "^($TITLE_TYPES)(\([^)]+\))?!?: .+" <<<"$title" ||
        { echo "not a conventional commit title: $title" >&2; return 1; }
}

step "pull request title" title_check
step "dependency licences" cargo deny check licenses
step "format" cargo fmt --all --check
step "lint" cargo clippy --all-targets -- -D warnings
step "tests" python3 scripts/test-parallel.py -- --workspace --all-targets
step "API docs" cargo doc --no-deps --workspace
# The site is built outside the checkout so the build leaves nothing behind.
site_dir="$(mktemp -d)"
trap 'rm -rf "$site_dir"' EXIT
step "docs site" mkdocs build --strict --quiet --site-dir "$site_dir"

# A check that changes or leaves files behind would mean the tested tree is not the merged one.
[ -z "$(git status --porcelain)" ] || die "the checks left changes in this checkout:
$(git status --porcelain)"

echo "land: every check passed on #$pr at ${head_sha:0:8} (macOS only; Linux tests run on GitHub after the merge)"
$check_only && exit 0

if ! $assume_yes; then
    printf 'land: type %s to squash-merge it into %s: ' "$pr" "$base"
    read -r answer
    [ "$answer" = "$pr" ] || die "not merged"
fi

# --match-head-commit: GitHub refuses if the branch moved since the checks ran.
gh pr merge "$pr" --squash --match-head-commit "$head_sha"
git fetch --quiet origin "$base"
echo "land: #$pr is merged; origin/$base is fetched"

# Tidy up. The main checkout is the first worktree git lists.
main_root="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
if [ "$here" = "$main_root" ]; then
    echo "land: $head is checked out in the main checkout; switch branch and delete it yourself"
    exit 0
fi
cd "$main_root"
# The checkout was clean when the checks finished; git refuses to remove it if that changed.
git worktree remove "$here"
echo "land: removed worktree $here"
# A squash merge leaves the branch unmerged in git's eyes, so the delete is forced.
git branch -D "$head" >/dev/null
echo "land: deleted local branch $head"
