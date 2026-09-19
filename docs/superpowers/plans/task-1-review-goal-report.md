# Task 1 Report: Forward Full Goal Instructions to Review in `superstate-core`

**Date:** 2026-09-19  
**Branch:** `feat/review-goal-forwarding`  
**Commit:** `6a6937f0634851355dd3d17d33312fcbe63017fa`  
**Status:** Completed successfully  

---

## 1. Executive Summary

In `superstate-core`, the adversarial code review runner (`superstate_review::execute_review`) was previously invoked with only the active milestone title (`Some(&active_task.title)`). For multi-milestone and complex objectives, the active milestone title is often too terse (e.g. `"task-1: Implement initial boilerplate"`) to give the adversarial reviewer adequate context regarding architectural constraints, requirements, and global invariants.

We updated `crates/superstate-core/src/daemon.rs` so that when `state.goal` is available, it is prioritized and passed to the code reviewer as the `task_instruction`. If `state.goal` is absent or unset, it safely falls back to `active_task.title`.

---

## 2. Test-Driven Development (TDD) Lifecycle

### 2.1 RED Phase: Failing Integration Test
Added `test_daemon_review_uses_full_state_goal_when_available` to `crates/superstate-core/tests/test_daemon_blocking_review.rs`:
- Configured a git repository with `.agent-state/` and driver artifacts in `.gitignore`.
- Initialized state with a comprehensive objective: `"Comprehensive Task Objective: Fix cython extension compatibility with Numpy 2.3"` and milestone title `"Short Task Title"`.
- Configured mock driver to capture the review prompt passed via stdin and assert presence of the comprehensive objective before emitting a blocking review report.
- Ran `cargo test -p superstate-core --test test_daemon_blocking_review test_daemon_review_uses_full_state_goal_when_available`.
- **Verified RED failure:**
  ```text
  thread 'test_daemon_review_uses_full_state_goal_when_available' panicked at
  'Prompt passed to review driver must contain full state goal in Task Instructions, got prompt:
  ...
  Task Instructions:
  Short Task Title
  ...'
  ```

### 2.2 GREEN Phase: Implementation
In `crates/superstate-core/src/daemon.rs` (around line 786):
```rust
let review_instruction = state.goal.as_deref().unwrap_or(&active_task.title);
if let Ok(report) = superstate_review::execute_review(
    &working_dir,
    &diff,
    Some(review_instruction),
    &review_cfg,
) {
```
- Ran `cargo test -p superstate-core --test test_daemon_blocking_review`.
- **Verified GREEN passing:** All 6 tests in `test_daemon_blocking_review` passed cleanly:
  - `test_daemon_config_default_review_blocking_false ... ok`
  - `test_daemon_blocking_review_fails_on_blocking_issues ... ok`
  - `test_daemon_advisory_review_without_blocking_advances_on_non_vacuous_gate ... ok`
  - `test_daemon_blocking_review_clean_report_advances ... ok`
  - `test_daemon_review_uses_full_state_goal_when_available ... ok`
  - `test_daemon_tracks_milestone_start_commit_across_retries ... ok`

---

## 3. Verification & Quality Gates

1. **Unit and Integration Suite:**
   - Ran `cargo test -p superstate-core`.
   - Result: All test binaries passed (`100% ok`, 0 failed, 0 errors).
2. **Static Analysis & Linting:**
   - Ran `cargo clippy --all-targets -- -D warnings`.
   - Result: `100% clean`, 0 warnings across all workspace crates.

---

## 4. Git Artifacts & Changes

- **Commit:** `6a6937f0634851355dd3d17d33312fcbe63017fa`
- **Subject:** `feat(daemon): forward full state goal to adversarial code review`
- **Files Modified:**
  - `crates/superstate-core/src/daemon.rs`: 3 lines changed (`+2, -1`)
  - `crates/superstate-core/tests/test_daemon_blocking_review.rs`: 117 lines changed (`+117`)
