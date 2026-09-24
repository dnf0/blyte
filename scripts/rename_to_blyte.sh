#!/usr/bin/env bash
# Rename the harness from superstate to blyte, in place, from the repository root.
# Repository names are NOT renamed: the GitHub repo (climate-x-org/superstate), the eval repo
# (superstate-eval, superstate_eval, superstate_eval_runs) and local checkout paths keep their names.
# Historical records (docs/superpowers/plans, docs/superpowers/specs, CHANGELOG.md entries) are left
# as written; only CHANGELOG.md's header is updated.
set -euo pipefail

test -f Cargo.toml && test -d crates/superstate-agent || { echo "run from the superstate repo root" >&2; exit 1; }

# 1. Move the crate directories.
for d in crates/superstate-*; do
  git mv "$d" "crates/blyte-${d#crates/superstate-}"
done

# 2. Rewrite text in tracked files, protecting repository names with placeholders.
files=$(git ls-files | grep -v -E '^docs/superpowers/(plans|specs)/|^CHANGELOG\.md$|\.lock$|^scripts/rename_to_blyte\.sh$' \
  | while read -r f; do [ -f "$f" ] && grep -Iq . "$f" && grep -l -i superstate "$f" || true; done)

for f in $files; do
  perl -pi -e '
    s#climate-x-org/superstate#\@\@REPO\@\@#g;
    s#repos/superstate#\@\@LOCAL\@\@#g;
    s#superstate-eval#\@\@EVALDASH\@\@#g;
    s#superstate_eval#\@\@EVALUS\@\@#g;
    s#SUPERSTATE#BLYTE#g;
    s#Superstate#Blyte#g;
    s#superstate#blyte#g;
    s#\@\@REPO\@\@#climate-x-org/superstate#g;
    s#\@\@LOCAL\@\@#repos/superstate#g;
    s#\@\@EVALDASH\@\@#superstate-eval#g;
    s#\@\@EVALUS\@\@#superstate_eval#g;
  ' "$f"
done

# 3. CHANGELOG.md: header only; past entries stay as recorded.
for f in CHANGELOG.md release-plz.toml; do
  if [ -f "$f" ]; then
    perl -pi -e 's/All notable changes to Superstate /All notable changes to Blyte (formerly Superstate) /' "$f"
  fi
done

# 4. Renamed imports sort differently.
cargo fmt --all

echo "renamed; now run: cargo build && cargo test --workspace --all-targets"
