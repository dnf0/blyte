# Task 3 Report: CLI Flag `--review-blocking` in `crates/superstate-cli`

**Status:** Completed  
**Commit SHA:** `fde4364`  
**Branch:** `feat/blocking-code-review`  
**Repository:** `/Users/danielfisher/repos/superstate`  
**Date:** 2026-09-17  

---

## Executive Summary

Task 3 implemented the `--review-blocking` CLI flag for the daemon and updated `cmd_review` in `crates/superstate-cli` to use `superstate_review::diff::get_milestone_diff` as specified in `docs/superpowers/plans/task-3-brief.md` and `docs/superpowers/plans/2026-09-17-advisory-review-diff-and-blocking.md`.

The implementation addresses the core user-facing and runner interfaces:
1. **`--review-blocking` CLI Flag**: Added `pub review_blocking: bool` to `DaemonArgs` in `crates/superstate-cli/src/state_cli.rs`, exposed across both `superstate state daemon` and `superstate daemon` subcommands with clear help documentation: `"Treat Critical and High severity review findings as blocking failures requiring retry"`.
2. **Daemon Invocation and Detachment Propagation**: Updated `cmd_daemon` in `crates/superstate-cli/src/commands/daemon.rs` to take `review_blocking: bool` and initialize `DaemonConfig` accordingly. When `--detach` is specified, `--review-blocking` is forwarded to the background child process. Updated all dispatch sites in `main.rs`, `state_cli.rs`, `handoff.rs`, and test fixtures.
3. **Milestone Diff Extraction in `superstate review`**: Updated `cmd_review` in `crates/superstate-cli/src/commands/review.rs` to use `superstate_review::diff::get_milestone_diff(&args.repo, None)`. When `superstate review` is invoked without explicit commit range arguments:
   - If the working tree is clean and 2+ commits exist, it automatically reviews `HEAD~1..HEAD` (the most recent commit).
   - If uncommitted or untracked changes exist, it diffs and reviews the working tree modifications.
   - If an explicit `--commit-range` is provided, it maintains behavior using `get_git_diff(&args.repo, Some(range))`.
4. **Binary Installation & Help Verification**: Reinstalled the release binary via `cargo install --path crates/superstate-cli --locked --force` and confirmed that `superstate state daemon --help`, `superstate daemon --help`, and `state daemon --help` expose `--review-blocking`.

---

## TDD Execution

### 1. Red Phase
Created `crates/superstate-cli/tests/test_cli_blocking_review.rs` with 3 targeted integration tests:
- `test_cli_daemon_help_has_review_blocking`: Validates that `superstate state daemon --help` and `superstate daemon --help` include `--review-blocking` and its descriptive text.
- `test_cli_daemon_accepts_review_blocking_flag`: Validates that `superstate state daemon --review-blocking` is parsed and accepted as a valid command line argument.
- `test_cli_review_without_args_reviews_last_commit_on_clean_working_tree`: Verifies that `superstate review` without args on a clean repo with multiple commits reviews `HEAD~1..HEAD` rather than exiting early with `"No changes detected in diff."`.

Ran test suite to capture RED failure:
```text
$ cargo test -p superstate --test test_cli_blocking_review
running 3 tests
test test_cli_daemon_help_has_review_blocking ... FAILED
test test_cli_daemon_accepts_review_blocking_flag ... FAILED
test test_cli_review_without_args_reviews_last_commit_on_clean_working_tree ... FAILED

failures:
---- test_cli_daemon_help_has_review_blocking stdout ----
`superstate state daemon --help` should mention --review-blocking

---- test_cli_daemon_accepts_review_blocking_flag stdout ----
CLI must accept --review-blocking, got stderr:
error: unexpected argument '--review-blocking' found

---- test_cli_review_without_args_reviews_last_commit_on_clean_working_tree stdout ----
superstate review without args should review HEAD~1..HEAD instead of clean early-exit, got:
{
  "summary": "No changes detected in diff.",
  "findings": [],
  "has_blocking_issues": false
}

test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.35s
```

### 2. Green Phase
- Updated `crates/superstate-cli/src/state_cli.rs`:
  - Added `pub review_blocking: bool` to `DaemonArgs`.
  - Passed `args.review_blocking` to `cmd_daemon` in `StateCommands::Daemon` handler.
- Updated `crates/superstate-cli/src/main.rs`:
  - Passed `args.review_blocking` to `cmd_daemon` in top-level `Commands::Daemon` handler.
- Updated `crates/superstate-cli/src/commands/daemon.rs`:
  - Added `review_blocking: bool` parameter to `cmd_daemon`.
  - Appended `cmd.arg("--review-blocking")` when `detach` is enabled.
  - Assigned `review_blocking` into `DaemonConfig`.
- Updated `crates/superstate-cli/src/commands/review.rs`:
  - Replaced diff fallback with `get_milestone_diff(&args.repo, None)`.
- Updated callers in `crates/superstate-cli/src/commands/handoff.rs` and `test_cli_daemon_lifecycle.rs`.

Ran `cargo test -p superstate --test test_cli_blocking_review`:
```text
running 3 tests
test test_cli_daemon_help_has_review_blocking ... ok
test test_cli_review_without_args_reviews_last_commit_on_clean_working_tree ... ok
test test_cli_daemon_accepts_review_blocking_flag ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s
```

Ran full workspace test suite `cargo test --all`:
- All tests across `superstate-core`, `superstate-review`, `superstate-worktree`, `superstate-mcp`, `superstate-rules`, `superstate-syntax`, and `superstate-cli` passed cleanly (0 failed).

### 3. Binary Installation & Verification
Installed binary:
```bash
cargo install --path crates/superstate-cli --locked --force
```

Verified output:
```text
$ superstate state daemon --help
Run autonomous background daemon over milestones

Usage: superstate state daemon [OPTIONS] [COMMAND]

Commands:
  list  List active and managed daemon processes
  help  Print this message or the help of the given subcommand(s)

Options:
  -s, --state <STATE>              [default: .agent-state/state.json]
      --driver <DRIVER>            Headless coding driver (e.g. claude, agy, or command)
      --detach                     Detach execution into a background daemon process
      --max-retries <MAX_RETRIES>  Maximum verification retries per milestone [default: 3]
      --timeout <TIMEOUT>          Timeout in seconds per verification attempt [default: 300]
      --advisory-review            Run advisory adversarial code review on milestone completion
      --review-blocking            Treat Critical and High severity review findings as blocking failures requiring retry
  -h, --help                       Print help
```

---

## File Changes Summary

| File | Change | Rationale |
|---|---|---|
| `crates/superstate-cli/src/state_cli.rs` | Modified | Add `review_blocking` to `DaemonArgs` and pass to `cmd_daemon` |
| `crates/superstate-cli/src/main.rs` | Modified | Pass `args.review_blocking` to `cmd_daemon` in top-level command |
| `crates/superstate-cli/src/commands/daemon.rs` | Modified | Add `review_blocking` parameter, detach argument forwarding, and `DaemonConfig` mapping |
| `crates/superstate-cli/src/commands/handoff.rs` | Modified | Update `cmd_daemon` call with default `review_blocking: false` |
| `crates/superstate-cli/src/commands/review.rs` | Modified | Use `get_milestone_diff(&args.repo, None)` for default diff extraction |
| `crates/superstate-cli/tests/test_cli_blocking_review.rs` | Created | Integration tests for `--review-blocking` CLI flag and review milestone diff |
| `crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs` | Modified | Update `cmd_daemon` test calls with `review_blocking: false` |

---

## Verification Evidence

- **Tests Passing:**
  - `cargo test -p superstate --test test_cli_blocking_review`: 3 passed, 0 failed.
  - `cargo test -p superstate`: All unit and integration tests passed.
  - `cargo test --all`: All workspace crates passed.
- **CLI Verification:**
  - `superstate state daemon --help` displays `--review-blocking`.
  - `superstate daemon --help` displays `--review-blocking`.
  - `state daemon --help` displays `--review-blocking`.
- **Git Commit:**
  - Commit `fde4364`: `feat(cli): wire --review-blocking CLI flag and milestone diff in review`.
