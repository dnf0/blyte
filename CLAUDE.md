# CLAUDE.md

## Working model: Claude orchestrates, agy implements

In this repo Claude does not implement plans itself. Claude writes specs and plans, dispatches each plan task to headless `agy` (Antigravity CLI) in its own worktree, validates the result independently, and merges it. Follow the `agy-orchestration` skill (`~/.claude/skills/agy-orchestration/`; repo rules in `.claude/agy-rules.md`) whenever executing a plan. Use it instead of `superpowers:subagent-driven-development` or `superpowers:executing-plans`.

Small fixes found during validation, plan edits and docs may be done directly.

## Repo basics

- Rust workspace (`crates/`); binary `blyte`.
- Checks (same as CI): `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Specs and plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/`.
- Worktrees live in `.wt/<task>` (gitignored), not the sibling-directory default.
- Conventional commits, `--no-gpg-sign`.
