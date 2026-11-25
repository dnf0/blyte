# CLAUDE.md

## Working model: Claude orchestrates; blyte implements

In this repo Claude does not implement plans itself. Claude writes specs and plans, dispatches each task with `blyte dispatch` (the `blyte-implementer` skill: one worktree per task under `.wt/`, independent tasks in parallel, this repository's `blyte.toml` as the gate; if it has none, run `blyte init` first), reviews each result with `blyte review`, records a verdict (`blyte review <id> --verdict accepted|fixed|redone`), then merges. Read-only research goes to blyte with the `blyte-research` skill. The implementer's rules are in `.claude/agy-rules.md`; blyte reads them itself. Use this instead of `superpowers:subagent-driven-development` or `superpowers:executing-plans`.

Small fixes found during validation, plan edits and docs may be done directly.

## Repo basics

- Rust workspace (`crates/`); binary `blyte`.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && python3 scripts/test-parallel.py -- --workspace --all-targets`
- Faster local test step: python3 scripts/test-changed.py tests only the crates a change affects.
- Pull requests are not checked on GitHub. `scripts/land.sh <pr>` runs every check on this machine (title, licences, format, lint, tests, docs), asks, then squash-merges; `scripts/land.sh --check <pr>` runs the checks only. GitHub runs the tests on Linux after each merge to `main`.
- Specs and plans live in `docs/specs/` and `docs/plans/`. `docs/superpowers/` holds the older ones, as written at the time.
- Worktrees live in `.wt/<task>` (gitignored), not the sibling-directory default.
- Conventional commits, `--no-gpg-sign`.
