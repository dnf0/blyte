# CLAUDE.md

## Working model: Claude orchestrates; blyte implements

In this repo Claude does not implement plans itself. Claude writes specs and plans, dispatches each task with `blyte dispatch` (the `blyte-implementer` skill: one worktree per task under `.wt/`, independent tasks in parallel, this repository's `blyte.toml` as the gate; if it has none, run `blyte init` first), reviews each result with `blyte review`, records a verdict (`blyte review <id> --verdict accepted|fixed|redone`), then merges. Read-only research goes to blyte with the `blyte-research` skill. The implementer's rules are in `.claude/agy-rules.md`; blyte reads them itself. Use this instead of `superpowers:subagent-driven-development` or `superpowers:executing-plans`.

Small fixes found during validation, plan edits and docs may be done directly.

## Repo basics

- Rust workspace (`crates/`); binary `blyte`.
- Checks (same as CI): `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Specs and plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/`.
- Worktrees live in `.wt/<task>` (gitignored), not the sibling-directory default.
- Conventional commits, `--no-gpg-sign`.
