# Task 1 Brief: Milestone Diff Extraction in `crates/superstate-review`

## Context
Working directory: `/Users/danielfisher/repos/superstate`
Branch: `feat/blocking-code-review`
Plan: `docs/superpowers/plans/2026-09-17-advisory-review-diff-and-blocking.md`

## What to Implement
Implement `get_milestone_diff` in `crates/superstate-review/src/diff.rs` and export it in `crates/superstate-review/src/lib.rs`.

Function signature:
```rust
pub fn get_milestone_diff(repo_root: &Path, start_commit: Option<&str>) -> Result<String, DiffError>
```

Behavior:
1. Determine `head_hash` via `git rev-parse HEAD`. If that fails (e.g. freshly initialized empty repo), fall back to `get_git_diff(repo_root, None)`.
2. If `start_commit` is provided:
   - Trim whitespace. If non-empty and `start != head_hash`, execute `git diff <start>..HEAD` to capture all commits made on the branch.
3. Next, check if there are any working tree changes (uncommitted or untracked) by running `get_git_diff(repo_root, Some("HEAD"))`. If non-empty, append to the diff string.
4. If `start_commit` is `None` and the combined diff is still empty, check if `HEAD~1` exists (`git rev-parse --verify HEAD~1`). If it exists, return `get_git_diff(repo_root, Some("HEAD~1..HEAD"))`.
5. Return `Ok(combined_diff)`.

## TDD Instructions
1. Write failing tests in `crates/superstate-review/tests/test_milestone_diff.rs` covering:
   - Committed changes only (working tree clean) with `Some(initial_hash)` -> returns committed diff.
   - Uncommitted changes only with `Some(initial_hash)` -> returns uncommitted diff.
   - Committed changes AND untracked new files with `Some(initial_hash)` -> returns both!
   - Clean working tree with `None` when `HEAD~1` exists -> returns `HEAD~1..HEAD` diff.
2. Run `cargo test -p superstate-review --test test_milestone_diff` and verify RED failure.
3. Implement `get_milestone_diff` in `crates/superstate-review/src/diff.rs` and export in `src/lib.rs`.
4. Run `cargo test -p superstate-review` and verify all tests pass GREEN.
5. Commit with `git commit --no-gpg-sign -m "feat(review): implement get_milestone_diff with committed and uncommitted change tracking"`.
6. Write full report to `docs/superpowers/plans/task-1-report.md`.
