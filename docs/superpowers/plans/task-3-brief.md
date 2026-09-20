# Task 3 Brief: Two-Tier Gate Execution in `DaemonRunner::step()`

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No compiler or clippy warnings (`cargo clippy --all-targets -- -D warnings`).
- Skip GPG verification on all commits (`--no-gpg-sign`).
- Zero prompt engineering: all behavior governed by git diffs, process exit codes (`$? == 0`), and state invariants.

## Task Details
- **Task:** Task 3: Two-Tier Gate Execution in `DaemonRunner::step()`
- **Depends on:** Task 1, Task 2
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-test-impact-slicing`

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs`
- Create: `crates/superstate-core/tests/test_daemon_impact_slicing.rs`

**Interfaces:**
- Consumes: `build_sliced_test_cmd`, `infer_impacted_tests`, `DaemonConfig.impact_slicing`
- Produces: Fast-fail pre-filter before `run_verification_cmd(verify_cmd, ...)` in `DaemonRunner::step()`

## Implementation Steps
Follow TDD strictly:
1. Create `crates/superstate-core/tests/test_daemon_impact_slicing.rs`:
   - `test_daemon_impact_slicing_fast_fail_skips_full_gate`:
     - Sets up a git repository with an initial commit.
     - Adds a source file `src/lib.py` and a failing test file `tests/test_lib.py` (which exits 1 when run).
     - Configures a milestone with `verify_cmd = "touch full_gate_ran.txt && exit 0"`.
     - Configures `DaemonConfig` with `impact_slicing: true`, `auto_rollback: false` (or true).
     - Runs `runner.step()`.
     - Verifies `DaemonStepOutcome::Failed` is returned.
     - Verifies `full_gate_ran.txt` was NOT created (proving the full gate was skipped on fast-fail).
     - Verifies `state.notes` contains an `[impact-fail]` entry.
   - `test_daemon_impact_slicing_passing_slice_executes_full_gate`:
     - Sets up a git repository with an initial commit.
     - Adds a source file `src/lib.py` and a passing test file `tests/test_lib.py`.
     - Configures a milestone with `verify_cmd = "touch full_gate_ran.txt && exit 0"`.
     - Configures `DaemonConfig` with `impact_slicing: true`.
     - Runs `runner.step()`.
     - Verifies `DaemonStepOutcome::Completed` or milestone advancement.
     - Verifies `full_gate_ran.txt` WAS created.
     - Verifies `state.notes` contains an `[impact-pass]` entry.
   - `test_daemon_impact_slicing_zero_impacted_falls_back_to_full_gate`:
     - Sets up a git repo where no test files match the diff.
     - Verifies daemon directly runs `verify_cmd` without error.
2. Run `cargo test -p superstate-core --test test_daemon_impact_slicing` to confirm failure (pre-filter not yet implemented).
3. Implement staged verification in `crates/superstate-core/src/daemon.rs`:
   - Import `use crate::impact::{build_sliced_test_cmd, infer_impacted_tests};`
   - In `DaemonRunner::step()` around line 755 (before `run_verification_cmd(verify_cmd, ...)`):
     - If `self.config.impact_slicing && is_inside_git_repo(&working_dir)`:
       - Query `start_commit = self.milestone_start_commits.get(&active_task.id)`.
       - Run `impacted = infer_impacted_tests(&working_dir, start_commit.map(|s| s.as_str()))`.
       - If `!impacted.is_empty()`:
         - Call `build_sliced_test_cmd(verify_cmd, &impacted, &working_dir)`.
         - If `Some(sliced_cmd)`:
           - Run `slice_res = run_verification_cmd(&sliced_cmd, &working_dir, self.config.timeout_secs)?`.
           - If `!slice_res.passed`:
             - Record `[impact-fail]` note in `state.notes`.
             - Save state atomically.
             - If `self.config.auto_rollback`, perform rollback.
             - Return `Ok(DaemonStepOutcome::Failed { task_id, error })`.
           - If `slice_res.passed`:
             - Record `[impact-pass]` note in `state.notes`.
             - Save state atomically.
             - Fall through to full `verify_cmd`.
4. Run `cargo test -p superstate-core --test test_daemon_impact_slicing` and verify PASS.
5. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
6. Commit changes:
   ```bash
   git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_impact_slicing.rs
   git commit --no-gpg-sign -m "feat(daemon): wire milestone test impact slicing into daemon verification loop"
   ```
7. Write implementer report to `docs/superpowers/plans/task-3-report.md`.
8. Report status: DONE / DONE_WITH_CONCERNS / BLOCKED.
