# Task 2 Report: Deterministic Daemon Rollback in `crates/superstate-core`

**Status:** Completed  
**Commit SHA:** `880cbd5`  
**Branch:** `feat/rapid-canary-eval-and-rollback`  
**Repository:** `/Users/danielfisher/repos/superstate`  
**Date:** 2026-09-18  

---

## Executive Summary

Task 2 implemented deterministic git rollback on verification and review failure within `crates/superstate-core` as specified in `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-2-rollback-brief.md`.

When autonomous agents attempt milestone implementation and fail their verification gate or code review quality gates, corrupted source files, half-baked edits, or untracked debris often accumulate in the workspace. In subsequent attempts, the agent builds upon corrupted states rather than clean baselines, leading to compounding failures and wasted turn budgets.

Deterministic daemon rollback ensures that whenever verification or blocking code review fails while retries remain:
1. The working tree is reset via `git reset --hard <start_commit>` back to the exact commit recorded when the milestone was started.
2. All untracked artifacts, partial compilations, and debris are cleaned via `git clean -fd`.
3. An epistemic note is appended to the ledger:
   `[rollback] Task {id}: attempt {attempt} failed verification; rolled back working tree to baseline commit {start_commit}. Avoid repeating previous failed approach.`
4. The workspace fingerprint is recomputed from the clean baseline so subsequent edits correctly register progress.
5. The state is saved atomically via `save_state_atomic`.

When `auto_rollback` is disabled (the default), existing behaviour is preserved without alteration.

---

## TDD Execution

### 1. Red Phase
Created `crates/superstate-core/tests/test_daemon_rollback.rs` containing two end-to-end integration tests:
- `test_daemon_rollback_on_verification_failure`: Verifies that with `auto_rollback: true`, a driver that edits tracked files and leaves untracked directories/files before failing verification triggers `DaemonStepOutcome::Failed`, restores `tracked.txt` to the baseline start commit, removes untracked files via `git clean -fd`, and records the formatted `[rollback]` note in `state.notes`.
- `test_daemon_no_rollback_when_disabled`: Verifies that with `auto_rollback: false`, modified tracked files and untracked artifacts are preserved on failure, and no `[rollback]` note is added.

Executed `cargo test -p superstate-core --test test_daemon_rollback` and verified RED compilation failure:
```text
error[E0560]: struct `DaemonConfig` has no field named `auto_rollback`
  --> crates/superstate-core/tests/test_daemon_rollback.rs:97:9
   |
97 |         auto_rollback: true,
   |         ^^^^^^^^^^^^^ `DaemonConfig` does not have this field
```

### 2. Green Phase
- **`DaemonConfig` Update**: Added `#[serde(default)] pub auto_rollback: bool` to `DaemonConfig` in `crates/superstate-core/src/daemon.rs` with default `false`.
- **Rollback Helper**: Added private helper `perform_rollback` on `DaemonRunner`:
  ```rust
  fn perform_rollback(
      &mut self,
      task_id: &str,
      attempt: usize,
      working_dir: &Path,
      state: &mut ExecutionState,
  ) -> Result<(), DaemonError>
  ```
  Executing `git reset --hard <start_commit>`, `git clean -fd`, pushing the `[rollback]` ledger entry, saving state atomically, and updating `self.last_fingerprint`.
- **Wiring into Failure Paths**:
  - In code review blocking gate check (`run_step()`, lines 782+): triggers rollback if `auto_rollback` is enabled and `attempts_so_far < attempt_ceiling`.
  - In gate verification failure (`run_step()`, lines 971+): triggers rollback before returning `DaemonStepOutcome::Failed`.
- **Borrow Scope Resolution**: Scoped the mutable borrow of `self.retry_counts` inside a block to calculate `current_retries`, ensuring `self` can be mutably borrowed by `perform_rollback` without borrow checker conflicts.
- **Construction Site Updates**: Updated `DaemonConfig` instantiations in `superstate-core` integration tests (`test_daemon_runner.rs`, `test_daemon_automerge.rs`, `test_daemon_blocking_review.rs`, `test_daemon_advisory_review.rs`, `test_driver_presets.rs`) and in `crates/superstate-cli/src/commands/daemon.rs`.

### 3. Verification & Passing Suites
Ran `cargo test -p superstate-core --test test_daemon_rollback`:
```text
running 2 tests
test test_daemon_no_rollback_when_disabled ... ok
test test_daemon_rollback_on_verification_failure ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.29s
```

Ran full `superstate-core` test suite:
`cargo test -p superstate-core` — 100% tests passed cleanly.

Ran workspace check:
`cargo check --all` — 0 errors, 0 warnings across all crates (`superstate-core`, `superstate-worktree`, `superstate-mcp`, `superstate-review`, `superstate-cli`).

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-core/src/daemon.rs` | Modified | Added `auto_rollback` to `DaemonConfig`, added `perform_rollback` helper, wired rollback on gate & review failures, scoped `retry_counts` borrow |
| `crates/superstate-core/tests/test_daemon_rollback.rs` | Created | Integration tests for rollback on verification failure and no-op when disabled |
| `crates/superstate-cli/src/commands/daemon.rs` | Modified | Added `auto_rollback: false` default initialization to maintain clean workspace compilation |
| `crates/superstate-core/tests/test_daemon_runner.rs` | Modified | Updated 18 `DaemonConfig` initialization sites with `auto_rollback: false` |
| `crates/superstate-core/tests/test_daemon_automerge.rs` | Modified | Updated 4 `DaemonConfig` initialization sites with `auto_rollback: false` |
| `crates/superstate-core/tests/test_daemon_blocking_review.rs` | Modified | Updated 4 `DaemonConfig` initialization sites with `auto_rollback: false` |
| `crates/superstate-core/tests/test_daemon_advisory_review.rs` | Modified | Updated 6 `DaemonConfig` initialization sites with `auto_rollback: false` |
| `crates/superstate-core/tests/test_driver_presets.rs` | Modified | Updated `DaemonConfig` initialization site with `auto_rollback: false` |

---

## Commit Details

- **Commit Message:** `feat(daemon): implement deterministic git rollback on verification failure`
- **Commit SHA:** `880cbd5cd3beba1afa5ca92e6f7caea9e1415e4d`
- **Files Committed:** 8 files changed, 324 insertions(+), 15 deletions(-)
- **GPG Signing:** Skipped (`--no-gpg-sign`) per repository requirements

---

## Next Steps

Proceed with **Task 3: CLI Flag & Integration in `superstate-cli`** (`docs/superpowers/plans/task-3-cli-rollback-brief.md`):
1. Add `--auto-rollback` flag to `DaemonArgs` in `crates/superstate-cli/src/args.rs`.
2. Wire `config.auto_rollback = args.auto_rollback` and detach flag propagation in `crates/superstate-cli/src/commands/daemon.rs`.
3. Add environment variable fallback `SUPERSTATE_AUTO_ROLLBACK=1|true`.
4. Add integration tests in `crates/superstate-cli/tests/test_daemon_cli.rs`.
