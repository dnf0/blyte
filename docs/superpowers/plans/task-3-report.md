# Task 3 Report: Daemon Advisory Review Loop in `crates/superstate-core`

## Summary
Successfully implemented Task 3 of the advisory code review system. Wired automated adversarial code review into the daemon execution loop (`DaemonRunner::run_step` and `DaemonRunner::step`) in `crates/superstate-core/src/daemon.rs` and added the `--advisory-review` CLI flag in `crates/superstate-cli/src/state_cli.rs`, `commands/daemon.rs`, and `main.rs`.

The loop enforces the critical architectural distinction between vacuous gates (e.g. `true`, `:`, `exit 0`, empty string) and non-vacuous verification gates:
- **Vacuous verification gate**: If the advisory code review flags blocking issues on a vacuous gate, the daemon rejects the completion, records a `[review-advisory]` note to `state.notes`, and returns `DaemonStepOutcome::Failed { reason: "advisory-review: blocking issues detected on vacuous gate", ... }` so the implementer driver receives the critique on the subsequent turn.
- **Non-vacuous verification gate**: If the verification gate is strict and passed, the daemon logs the `[review-advisory]` note into the ledger for diagnostic context but allows the milestone to advance, avoiding the 60.5% benchmark false-alarm trap.

## Changes Made
1. **`crates/superstate-core/Cargo.toml`**:
   - Added dependency: `superstate-review = { path = "../superstate-review" }`.

2. **`crates/superstate-core/src/daemon.rs`**:
   - Added `pub advisory_review: bool` (with `#[serde(default)]`) to `DaemonConfig`.
   - Implemented `Default` for `DaemonConfig` (defaulting `advisory_review: false`, `advisory_gate: false`, `max_retries: 3`, `timeout_secs: 300`).
   - Added `pub fn step(&mut self) -> Result<DaemonStepOutcome, DaemonError>` alias on `DaemonRunner` forwarding to `self.run_step()`.
   - Wired advisory review into `run_step()` right after verification command passes:
     - Checks `if gate_res.passed && self.config.advisory_review && working_dir.join(".git").exists()`.
     - Extracts diff using `superstate_review::get_git_diff(&working_dir, None)`.
     - Executes review with `superstate_review::execute_review(&working_dir, &diff, None, &ReviewConfig { driver: self.config.driver_cmd.clone(), timeout_secs: self.config.timeout_secs })`.
     - When blocking issues exist (`report.has_blocking_issues`):
       - Appends `[review-advisory] Task <id>: <summary>. Problem: <problem>` to `state.notes` via `LedgerEntry::new("superstate-daemon", ...)` and saves state atomically.
       - If `is_vacuous_gate(verify_cmd)` and `driver_cmd.is_some()` and under attempt ceiling: returns `DaemonStepOutcome::Failed` with reason `"advisory-review: blocking issues detected on vacuous gate"`.
       - If non-vacuous gate: continues to milestone completion.

3. **`crates/superstate-cli/src/state_cli.rs`**:
   - Added `#[arg(long)] pub advisory_review: bool` to `DaemonArgs`.
   - Updated `StateCommands::Daemon` handler to pass `args.advisory_review` to `commands::daemon::cmd_daemon`.

4. **`crates/superstate-cli/src/commands/daemon.rs`**:
   - Updated `cmd_daemon` signature to accept `advisory_review: bool`.
   - Forwarded `--advisory-review` flag when spawning detached daemon process.
   - Populated `advisory_review` in in-process `DaemonConfig`.

5. **`crates/superstate-cli/src/main.rs`**:
   - Forwarded `args.advisory_review` in top-level `Commands::Daemon` match arm.

6. **Workspace Updates & Test Consistency**:
   - Updated call sites of `cmd_daemon` in `crates/superstate-cli/src/commands/handoff.rs` and `crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs`.
   - Updated all `DaemonConfig` initializations in `crates/superstate-core/tests/test_daemon_runner.rs`, `crates/superstate-core/tests/test_daemon_automerge.rs`, and `crates/superstate-core/tests/test_driver_presets.rs`.

7. **`crates/superstate-core/tests/test_daemon_advisory_review.rs`**:
   - Added comprehensive integration tests:
     - `test_daemon_config_default_advisory_review_false`: verifies default config value is `false`.
     - `test_daemon_advisory_review_clean_diff_advances`: verifies clean diff with no changes advances cleanly.
     - `test_daemon_advisory_review_blocking_issues_vacuous_gate_fails`: verifies mock driver returning blocking issues on a vacuous gate (`verify_cmd: "true"`) fails step with `"advisory-review: blocking issues detected on vacuous gate"` and records `[review-advisory]` to `state.notes`.
     - `test_daemon_advisory_review_blocking_issues_non_vacuous_gate_advances`: verifies mock driver returning blocking issues on a non-vacuous gate (`verify_cmd: "test -f marker.txt"`) records `[review-advisory]` note to `state.notes` and advances milestone.

## TDD Verification

### RED Phase
Executed `cargo test -p superstate-core --test test_daemon_advisory_review`:
```text
error[E0599]: no function or associated item named `default` found for struct `DaemonConfig`
error[E0560]: struct `DaemonConfig` has no field named `advisory_review`
error[E0599]: no method named `step` found for struct `DaemonRunner`

error: could not compile `superstate-core` (test "test_daemon_advisory_review") due to 5 previous errors
```
Tests failed as expected due to missing config field, missing default trait implementation, and missing step method.

### GREEN Phase
Executed `cargo test -p superstate-core --test test_daemon_advisory_review`:
```text
running 4 tests
test test_daemon_config_default_advisory_review_false ... ok
test test_daemon_advisory_review_blocking_issues_vacuous_gate_fails ... ok
test test_daemon_advisory_review_clean_diff_advances ... ok
test test_daemon_advisory_review_blocking_issues_non_vacuous_gate_advances ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s
```

### Full Workspace & Quality Checks
- `cargo test --all`: All workspace test suites passed across all crates (`superstate-core`, `superstate-cli`, `superstate-mcp`, `superstate-review`, `superstate-rules`, `superstate-syntax`, `superstate-worktree`).
- `cargo clippy --all`: Passed cleanly with zero warnings.

## Commits Created
- `0a0d763`: `feat(daemon): wire advisory review into milestone gate evaluation`

## Notes & Concerns
- None. Vacuous vs. non-vacuous gate semantics are completely honored and covered by automated integration tests.
