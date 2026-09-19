# Task 2 Brief: Daemon Runner Topological Advancement & Scoped Rollback

## Working Directory
`/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-dag-execution`
Branch: `feat/daemon-dag-execution`

## Goal
Wire `DaemonRunner` in `crates/superstate-core/src/daemon.rs` to support multi-milestone DAG execution:
1. Handle `advance_step()` deadlock errors gracefully by transitioning state to `ExecutionStatus::Blocked`, appending a `[dag-blocked]` ledger entry, and returning `DaemonStepOutcome::Blocked`.
2. Ensure milestone checkpoint commits and baseline commit tracking cleanly isolate `--auto-rollback` resets to the active milestone, so rolling back Milestone 2 never loses or mutates Milestone 1's completed work.
3. Verify sequential, multi-attempt rollback, and deadlock handling in end-to-end integration tests.

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies.
- Zero network I/O or direct API keys in Rust binary.
- Use `--no-gpg-sign` for git commits.
- Follow strict Test-Driven Development (TDD).

## Requirements

### 1. `crates/superstate-core/src/daemon.rs`
In `run_step()`:
- When verification passes and `self.step_advancer` is `None`:
  Call `advance_step(&mut state)`.
  - On `Ok(())`:
    - Save updated state.
    - If `state.status == ExecutionStatus::Completed`, finalize execution (`finalize_completed_execution`).
    - Emit `DaemonStepOutcome::Advanced { completed_task_id: active_task.id, next_task_id: state.active_task.as_ref().map(|t| t.id.clone()) }`.
  - On `Err(StateError::InvalidTransition(msg))`:
    - Transition `state.status = ExecutionStatus::Blocked`.
    - Record note `LedgerEntry::new("superstate-daemon", format!("[dag-blocked] Milestone {} completed, but remaining tasks cannot advance: {}", active_task.id, msg))`.
    - `save_state_atomic(&self.config.state_path, &state)?;`
    - Emit `DaemonStepOutcome::Blocked { task_id: active_task.id, reason: msg }`.
  - On other `Err(e)`:
    - Return `Err(DaemonError::State(e))`.

### 2. Integration Tests: `crates/superstate-core/tests/test_daemon_dag_execution.rs`
Write 3 tests:
1. `test_daemon_executes_sequential_milestones_with_checkpoints`:
   - 2-milestone DAG (task-1 -> task-2).
   - Mock driver creates `a.txt` for task-1 and `b.txt` for task-2.
   - Run `runner.run_to_completion()`.
   - Verify report status is `"completed"`. Both `a.txt` and `b.txt` exist.
   - Verify git log contains checkpoint commits for both `task-1` and `task-2`.
2. `test_daemon_rollback_isolation_preserves_earlier_milestone_commits`:
   - 2-milestone DAG (task-1 -> task-2).
   - Mock driver creates `a.txt` for task-1 (passes).
   - On task-2 attempt 1: writes `bad.txt` (fails verification). Daemon triggers rollback.
   - Verify `a.txt` is STILL present in repo, while `bad.txt` is removed.
   - On task-2 attempt 2: writes `b.txt` (passes).
   - Final state is completed with both `a.txt` and `b.txt`, and no `bad.txt`.
3. `test_daemon_deadlock_halts_with_blocked_outcome`:
   - Task 1 passes, but pending Task 2 depends on non-existent `task-99`.
   - Daemon advances Task 1, detects deadlock in remaining tasks, records `[dag-blocked]` note, and transitions state to `Blocked`.
   - `run_step()` returns `DaemonStepOutcome::Blocked`.

## Verification
- `cargo test -p superstate-core --test test_daemon_dag_execution`
- `cargo test -p superstate-core`
- `cargo check --all`

## Completion
- Commit with: `git commit --no-gpg-sign -m "feat(daemon): wire DAG milestone transitions, checkpoint commits, and scoped rollback"`
- Write detailed report to `docs/superpowers/plans/task-2-report.md`.
