# Task 3 Fix Brief: Address Reviewer Findings in `crates/superstate-core/src/daemon.rs`

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No compiler or clippy warnings (`cargo clippy --all-targets -- -D warnings`).
- Skip GPG verification on all commits (`--no-gpg-sign`).
- Zero prompt engineering: all behavior governed by git diffs, process exit codes (`$? == 0`), and state invariants.

## Issues to Address

### 1. Critical: Infinite Loop Hazard on Persistent Fast Slice Failures
In `crates/superstate-core/src/daemon.rs` around lines 785-820:
When `!slice_res.passed`:
Check if retries are exhausted:
```rust
let attempts_spent = self.attempts.get(&active_task.id).copied().unwrap_or(0);
let retries_remaining = self.config.max_retries.saturating_sub(attempts_spent);

if retries_remaining == 0 {
    if self.config.advisory_gate {
        state.notes.push(LedgerEntry::new(
            "superstate-daemon",
            format!(
                "[impact-advisory] Task {}: Fast slice failed but advisory_gate is enabled; proceeding to milestone completion",
                active_task.id
            ),
        ));
        // Allow fall-through or handle advisory advancement
    } else {
        state.status = ExecutionStatus::Blocked;
        state.errors.push(format!(
            "Task '{}' blocked: fast test slice failed and max retries ({}) exhausted: {}",
            active_task.id, self.config.max_retries, bounded_diag
        ));
        save_state_atomic(&self.config.state_path, &state)?;
        return Ok(DaemonStepOutcome::Blocked {
            task_id: active_task.id.clone(),
            reason: format!(
                "Fast test slice failed and max retries ({}) exhausted: {}",
                self.config.max_retries, bounded_diag
            ),
        });
    }
}
```

### 2. Integration Test:
In `crates/superstate-core/tests/test_daemon_impact_slicing.rs`:
Add test `test_daemon_impact_slicing_fast_fail_exhausts_retries_and_blocks`:
- Set `config.max_retries = 1`.
- Set up a failing test slice and run step twice (or run until retries exhaust).
- Assert that `step()` returns `DaemonStepOutcome::Blocked` and `state.status == ExecutionStatus::Blocked`.

## Implementation Steps
1. Add the test in `test_daemon_impact_slicing.rs`.
2. Update `daemon.rs` to handle retry exhaustion and `Blocked` transition on fast-fail.
3. Run `cargo test -p superstate-core --test test_daemon_impact_slicing` and confirm all tests pass.
4. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
5. Commit using `git commit --no-gpg-sign -m "fix(daemon): handle retry exhaustion and blocked state on fast slice failures"`.
6. Write report to `docs/superpowers/plans/task-3-fix-report.md`.
