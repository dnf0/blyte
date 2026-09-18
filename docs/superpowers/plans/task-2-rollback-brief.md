# Task 2 Brief: Deterministic Daemon Rollback in `crates/superstate-core`

## Context & Purpose
Working directory: `/Users/danielfisher/repos/superstate`
Branch: `feat/rapid-canary-eval-and-rollback`

Currently, when a driver attempt fails verification or code review blocks, `DaemonRunner` appends a `[verify-fail]` note and lets the driver attempt another turn on the dirty, corrupted working tree. This causes compounding errors.
We are implementing **Deterministic Daemon Rollback**: restoring the working tree to the milestone start commit via `git reset --hard` and `git clean -fd` upon failure, before the next retry attempt begins.

## Requirements
1. **`DaemonConfig` Update** (`crates/superstate-core/src/daemon.rs`):
   - Add field `pub auto_rollback: bool` to `DaemonConfig`.
   - Update `DaemonConfig::default()` or construction sites if any.
2. **State Machine Execution in `DaemonRunner::step()`**:
   - In `step()`, when a milestone begins, `self.milestone_start_commits.insert(active_task.id.clone(), head)` is already recorded.
   - When verification fails (`!gate_res.passed` or review blocks) AND `attempts_so_far < attempt_ceiling`:
     - If `self.config.auto_rollback && is_inside_git_repo(&working_dir)`:
       - Look up `start_commit` in `self.milestone_start_commits.get(&active_task.id)`.
       - If found:
         - Execute `git reset --hard <start_commit>` in `working_dir`.
         - Execute `git clean -fd` in `working_dir`.
         - Append an epistemic note to `state.notes`:
           `[rollback] Task {id}: attempt {attempt} failed verification; rolled back working tree to baseline commit {start_commit}. Avoid repeating previous failed approach.`
         - Persist state atomically (`save_state_atomic(&self.config.state_path, &state)`).
3. **Deterministic Integration Test** (`crates/superstate-core/tests/test_daemon_rollback.rs`):
   - Write a unit/integration test verifying that when `auto_rollback: true` and the driver modifies files and creates untracked files and fails verification:
     - The daemon step returns `DaemonStepOutcome::Failed`.
     - The modified files are rolled back to the initial commit content.
     - The untracked files are deleted.
     - `state.notes` contains the `[rollback]` note.
4. **Compile & Verification**:
   - `cargo test -p superstate-core` must pass cleanly.
   - `cargo check --all` must pass with zero warnings.
5. **Commit**:
   - Commit with message `feat(daemon): implement deterministic git rollback on verification failure` (use `--no-gpg-sign`).
6. **Report**:
   - Write full report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-2-rollback-report.md`.
