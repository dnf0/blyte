# Task 2 Brief: Milestone Base Commit Tracking & `--review-blocking` in `crates/superstate-core`

## Context
Working directory: `/Users/danielfisher/repos/superstate`
Branch: `feat/blocking-code-review`
Plan: `docs/superpowers/plans/2026-09-17-advisory-review-diff-and-blocking.md`

## What to Implement

In `crates/superstate-core/src/daemon.rs`:

1. **`DaemonConfig` Addition**:
   Add `pub review_blocking: bool` to `DaemonConfig` (default: `false`).

2. **Track Milestone Base Commits in `DaemonRunner`**:
   Add field `milestone_start_commits: HashMap<String, String>` to `DaemonRunner`.
   Initialize it empty in `DaemonRunner::new`.
   In `DaemonRunner::step`:
   - When an `active_task` is running (before or after driver turn):
     If `milestone_start_commits` does not contain `active_task.id`:
     Resolve current `HEAD` commit via `git rev-parse HEAD` in `working_dir` and insert it into `self.milestone_start_commits.insert(active_task.id.clone(), head_hash)`.
   - When a milestone successfully finishes or is marked completed/cleaned up:
     Remove `active_task.id` from `self.milestone_start_commits`.

3. **Wire `get_milestone_diff` and Review Blocking**:
   In `DaemonRunner::step`:
   When `gate_res.passed` is true and `(self.config.advisory_review || self.config.review_blocking)` is true:
   - Retrieve `start_commit` from `self.milestone_start_commits.get(&active_task.id).map(|s| s.as_str())`.
   - Call `superstate_review::diff::get_milestone_diff(&working_dir, start_commit)` instead of the old `get_git_diff(&working_dir, Some("HEAD"))`.
   - Execute review via `superstate_review::runner::execute_review`.
   - If findings exist, record them to `state.notes` as before.
   - **Enforce Blocking**:
     ```rust
     let should_block = report.has_blocking_issues && (
         self.config.review_blocking || is_vacuous_gate(verify_cmd)
     );
     
     if should_block && self.config.driver_cmd.is_some() && attempts_so_far < attempt_ceiling {
         *self.total_attempts.entry(active_task.id.clone()).or_insert(0) += 1;
         return Ok(DaemonStepOutcome::Failed {
             task_id: active_task.id,
             reason: format!("code-review: {} blocking issue(s) detected", report.findings.len()),
             retries_remaining: self.config.max_retries.saturating_sub(attempts_so_far),
         });
     }
     ```
   - If `!should_block` (either review was clean, or findings were non-blocking, or `--review-blocking` was false on a non-vacuous gate):
     Clear failures, commit/auto-merge if configured, and mark task completed as usual.

## TDD Instructions
1. Write failing integration tests in `crates/superstate-core/tests/test_daemon_blocking_review.rs`:
   - Test 1: Milestone on non-vacuous gate with committed code changes and `--review-blocking`. Mock review returns `has_blocking_issues: true`. Verify `DaemonStepOutcome::Failed` with reason containing `"code-review"`.
   - Test 2: Milestone on non-vacuous gate with `--review-blocking`. Mock review returns clean report. Verify `DaemonStepOutcome::TaskCompleted`.
   - Test 3: Milestone on non-vacuous gate with `--advisory-review` (and `review_blocking: false`). Mock review returns `has_blocking_issues: true`. Verify note is appended to `state.notes`, but outcome is `DaemonStepOutcome::TaskCompleted`.
2. Run `cargo test -p superstate-core --test test_daemon_blocking_review` and verify RED failure.
3. Implement in `crates/superstate-core/src/daemon.rs`. Update existing tests if `DaemonConfig` construction needs `review_blocking: false`.
4. Run `cargo test -p superstate-core` and verify all tests pass GREEN.
5. Commit with conventional commit message `feat(daemon): track milestone start commit and support review_blocking` (skip GPG with `--no-gpg-sign`).
6. Write full report to `docs/superpowers/plans/task-2-report.md`.
