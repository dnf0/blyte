# Task 2 Brief: CLI Command `superstate review` in `crates/superstate-cli`

- **Repository Directory**: `/Users/danielfisher/repos/superstate`
- **Current Branch**: `feat/advisory-code-review`
- **Depends on:** Task 1

## Goal
Implement the full CLI command `superstate review` in `crates/superstate-cli`, replacing the `ReviewReport::clean(...)` stub with real execution via `superstate_review::execute_review`. Support flags for commit ranges, driver overrides, JSON output, ledger recording, and exit-on-blocking.

## Files to Touch
- Modify: `crates/superstate-cli/src/main.rs`
- Modify: `crates/superstate-cli/src/commands/review.rs`
- Create: `crates/superstate-cli/tests/test_cli_review.rs`

## Detailed Requirements & Interfaces
1. In `crates/superstate-cli/src/main.rs`:
   Update `Commands::Review`:
   ```rust
   Review {
       /// Git commit range (e.g. HEAD~1..HEAD). If omitted, inspects uncommitted diff in working tree
       commit_range: Option<String>,
       /// Repository root path
       #[arg(short, long, default_value = ".")]
       repo: PathBuf,
       /// Reviewer driver command (e.g. "agy --model gemini-3.8-flash-low" or "claude")
       #[arg(short, long)]
       driver: Option<String>,
       /// Path to .agent-state/state.json (auto-detected if omitted)
       #[arg(long)]
       state_path: Option<PathBuf>,
       /// Record critical/high findings into the state epistemic ledger
       #[arg(long)]
       record: bool,
       /// Output full ReviewReport JSON to stdout
       #[arg(long)]
       json: bool,
       /// Exit with code 1 if blocking issues are present
       #[arg(long)]
       fail_on_blocking: bool,
   }
   ```
2. In `crates/superstate-cli/src/commands/review.rs`:
   Implement:
   ```rust
   pub struct ReviewArgs {
       pub commit_range: Option<String>,
       pub repo: PathBuf,
       pub driver: Option<String>,
       pub state_path: Option<PathBuf>,
       pub record: bool,
       pub json: bool,
       pub fail_on_blocking: bool,
   }

   pub fn cmd_review(args: ReviewArgs) -> Result<(), String>;
   ```
   - Diff Extraction:
     Call `superstate_review::get_git_diff(&args.repo, args.commit_range.as_deref())`.
     Note: If `args.commit_range` is `None` and `get_git_diff` with `None` returns empty because changes are unstaged or staged vs HEAD, ensure `get_git_diff` captures both or `HEAD` comparison if a git repo has commits. Check how `get_git_diff` works: `git diff [range]`. If `range` is `None`, `git diff` shows unstaged changes. If unstaged is empty, `git diff HEAD` shows staged + unstaged if HEAD exists. Let's make sure it handles uncommitted changes cleanly.
   - If diff is empty:
     - If `--json`, print JSON `ReviewReport::clean("No changes detected in diff.")`.
     - Else print "No changes detected in diff. Review clean."
     - Return `Ok(())`.
   - Execution:
     Configure `ReviewConfig { driver: args.driver, timeout_secs: 120 }`.
     Call `execute_review(&args.repo, &diff, None, &config)`.
   - Recording to Ledger:
     If `args.record` is true and `report.has_blocking_issues` is true (or if findings exist):
     - Resolve state file path (from `args.state_path` or looking for `.agent-state/state.json` in `args.repo`).
     - If state file exists, for each finding (or summarizing blocking findings):
       Append to state ledger using `superstate_core::ledger::append_ledger_note` or `state.notes.push(LedgerEntry::new("reviewer", ...))` and save atomic.
       Format: `[review-advisory] {finding.file}:{range.0}-{range.1}: {finding.problem} (Fix: {finding.suggested_fix})`.
   - Output:
     - If `args.json`: print `serde_json::to_string_pretty(&report)`.
     - Else: print `report` using `Display` implementation.
   - Exit Code:
     - If `args.fail_on_blocking` && `report.has_blocking_issues`: return `Err("Review failed: blocking issues detected".to_string())`.
     - Otherwise `Ok(())`.

3. Tests (`crates/superstate-cli/tests/test_cli_review.rs`):
   - Test empty diff output and `--json`.
   - Test mock driver execution with `--json`.
   - Test `--record` appending notes to `.agent-state/state.json`.
   - Test `--fail-on-blocking` exiting with error when blocking issues exist.

## TDD Instructions
- Step 1: Write failing tests in `crates/superstate-cli/tests/test_cli_review.rs`.
- Step 2: Run `cargo test -p superstate-cli --test test_cli_review` (verify FAIL).
- Step 3: Implement `ReviewArgs` and `cmd_review` in `review.rs`, update `main.rs`.
- Step 4: Run `cargo test -p superstate-cli` (verify PASS).
- Step 5: Commit changes with message `feat(cli): wire superstate review with driver execution, --json and --record`.
