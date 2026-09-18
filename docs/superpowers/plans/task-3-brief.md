# Task 3 Brief: CLI Flag `--review-blocking` in `crates/superstate-cli`

## Context
Working directory: `/Users/danielfisher/repos/superstate`
Branch: `feat/blocking-code-review`
Plan: `docs/superpowers/plans/2026-09-17-advisory-review-diff-and-blocking.md`

## What to Implement

1. **CLI Flag `--review-blocking` for Daemon**:
   In `crates/superstate-cli/src/main.rs`:
   Add `--review-blocking` argument to `StateCommands::Daemon`:
   ```rust
   #[arg(long, help = "Treat Critical and High severity review findings as blocking failures requiring retry")]
   review_blocking: bool,
   ```
   Pass `review_blocking` to `cmd_daemon` in `crates/superstate-cli/src/commands/daemon.rs`.
   In `cmd_daemon`, assign `review_blocking: args.review_blocking` into `DaemonConfig`.

2. **Update `cmd_review` in `crates/superstate-cli/src/commands/review.rs`**:
   Replace the old diff extraction logic with `superstate_review::diff::get_milestone_diff`:
   ```rust
   let diff = match args.commit_range.as_deref() {
       Some(range) => get_git_diff(&args.repo, Some(range))
           .map_err(|e| format!("Failed to get git diff: {}", e))?,
       None => {
           get_milestone_diff(&args.repo, None)
               .map_err(|e| format!("Failed to get git diff: {}", e))?
       }
   };
   ```
   This ensures `superstate review` without arguments automatically reviews the last commit (`HEAD~1..HEAD`) if working tree is clean, or working tree + untracked files if dirty!

3. **Install and Verify**:
   - Write integration tests in `crates/superstate-cli/tests/test_cli_blocking_review.rs` verifying `--review-blocking` flag is parsed and passed into daemon runner.
   - Run `cargo test -p superstate-cli`.
   - Install binary: `cargo install --path crates/superstate-cli --locked --force`.
   - Verify `superstate state daemon --help` outputs `--review-blocking`.

## TDD Instructions
1. Write failing integration tests in `crates/superstate-cli/tests/test_cli_blocking_review.rs`.
2. Run `cargo test -p superstate-cli --test test_cli_blocking_review` and verify RED failure.
3. Implement in `crates/superstate-cli/src/main.rs`, `crates/superstate-cli/src/commands/daemon.rs`, and `crates/superstate-cli/src/commands/review.rs`.
4. Run `cargo test -p superstate-cli` and verify all tests pass GREEN.
5. Reinstall CLI binary (`cargo install --path crates/superstate-cli --locked --force`).
6. Commit with conventional commit message `feat(cli): wire --review-blocking CLI flag and milestone diff in review` (skip GPG with `--no-gpg-sign`).
7. Write full report to `docs/superpowers/plans/task-3-report.md`.
