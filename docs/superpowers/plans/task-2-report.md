# Task 2 Report: CLI Command `superstate review` in `crates/superstate-cli`

## Summary
Successfully implemented Task 2 of the advisory code review system. Replaced the stub `ReviewReport::clean(...)` in `crates/superstate-cli/src/commands/review.rs` and wired `Commands::Review` in `crates/superstate-cli/src/main.rs` with driver execution, working tree / commit-range diff detection, JSON output, epistemic ledger recording, and fail-on-blocking support.

## Changes Made
1. **`crates/superstate-cli/src/main.rs`**:
   - Updated `Commands::Review` clap enum variant with all required flags:
     - Positional `commit_range: Option<String>`
     - `--repo <PathBuf>` (default: `"."`)
     - `--driver <String>` (custom reviewer driver command)
     - `--state-path <PathBuf>` (explicit path to `.agent-state/state.json`)
     - `--record` (record advisory findings to epistemic ledger)
     - `--json` (format output as JSON `ReviewReport`)
     - `--fail-on-blocking` (exit with code 1 if blocking issues exist)
   - Updated `main()` command match to construct `ReviewArgs` and dispatch to `commands::review::cmd_review`.

2. **`crates/superstate-cli/src/commands/review.rs`**:
   - Defined `pub struct ReviewArgs`.
   - Implemented `pub fn cmd_review(args: ReviewArgs) -> Result<(), String>`.
   - Git Diff extraction:
     - When `commit_range` is provided, executes `get_git_diff(&args.repo, Some(range))`.
     - When `commit_range` is omitted, checks `get_git_diff(&args.repo, Some("HEAD"))` to capture both staged and unstaged working tree diffs relative to HEAD, falling back to `get_git_diff(&args.repo, None)` if HEAD is not yet present.
   - Handled empty diffs:
     - If `--json`, outputs JSON serialized `ReviewReport::clean("No changes detected in diff.")`.
     - Else outputs `"No changes detected in diff. Review clean."`.
   - Configured and executed `execute_review(&args.repo, &diff, None, &config)` with 120s default timeout and driver override.
   - Implemented `--record`:
     - Resolves state file path using `resolve_state_path` against `args.state_path` or default `.agent-state/state.json`.
     - Appends findings to epistemic ledger via `append_ledger_note` using author `"reviewer"` and format: `[review-advisory] {finding.file}:{range.0}-{range.1}: {finding.problem} (Fix: {finding.suggested_fix})`.
   - Implemented output formatting:
     - If `--json`: pretty-printed JSON.
     - Else: human-readable report via `Display` implementation.
   - Handled `--fail-on-blocking`: returns `Err("Review failed: blocking issues detected".to_string())` if `args.fail_on_blocking && report.has_blocking_issues`.

3. **`crates/superstate-cli/tests/test_cli_review.rs`**:
   - Created comprehensive integration tests:
     - `test_review_empty_diff_text_and_json`: tests text and `--json` outputs when diff is clean.
     - `test_review_mock_driver_with_json`: tests mock driver returning structured review findings with `--json`.
     - `test_review_record_to_ledger`: tests `--record` recording advisory notes to `.agent-state/state.json` via `read_ledger`.
     - `test_review_fail_on_blocking`: tests `--fail-on-blocking` exiting non-zero when blocking issues exist, and succeeding without the flag.
     - `test_review_explicit_commit_range`: tests commit range arguments (e.g. `HEAD~1..HEAD`).
     - `test_review_explicit_state_path`: tests `--state-path` explicit override with `--record`.

## TDD Verification

### RED Phase
Executed `cargo test -p superstate --test test_cli_review`:
```text
failures:
    test_review_empty_diff_text_and_json
    test_review_fail_on_blocking
    test_review_mock_driver_with_json
    test_review_record_to_ledger

test result: FAILED. 0 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.61s
```
All tests failed as expected due to missing CLI flags and stub logic.

### GREEN Phase
Executed `cargo test -p superstate --test test_cli_review`:
```text
running 6 tests
test test_review_empty_diff_text_and_json ... ok
test test_review_mock_driver_with_json ... ok
test test_review_explicit_commit_range ... ok
test test_review_record_to_ledger ... ok
test test_review_explicit_state_path ... ok
test test_review_fail_on_blocking ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.67s
```

### Full Workspace & Quality Checks
- `cargo test --workspace`: All workspace tests passing across `superstate-core`, `superstate-review`, `superstate-rules`, `superstate-syntax`, `superstate-worktree`, `superstate-mcp`, and `superstate` (0 failures).
- `cargo clippy -p superstate`: Clean (0 warnings).
- `cargo fmt --check`: Clean (0 formatting diffs).

## Commits Created
- `d9de3a3`: `feat(cli): wire superstate review with driver execution, --json and --record`

## Next Steps
Task 2 is complete. Proceed to subsequent tasks in the plan or integration with workflow drivers.
