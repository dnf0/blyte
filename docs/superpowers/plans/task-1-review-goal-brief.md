# Task 1 Brief: Forward Full Goal Instructions to Review in `superstate-core`

## Workspace & Context
- Repo: `/Users/danielfisher/repos/superstate`
- Current Branch: `docs/adversarial-review-spec` (switch to `feat/review-goal-forwarding` based on `main` or current branch)

## What to Implement
Follow TDD strictly:
1. In `crates/superstate-core/tests/test_daemon_blocking_review.rs`:
   Add a test `test_daemon_review_uses_full_state_goal_when_available`:
   - Setup a test git repo with state containing a long goal (`"Comprehensive Task Objective: Fix cython extension compatibility with Numpy 2.3"`).
   - Commit a broken file.
   - Run runner with a mock driver that asserts `"Comprehensive Task Objective"` is present in the prompt passed via stdin and returns a blocking review finding.
   - Run `cargo test -p superstate-core --test test_daemon_blocking_review test_daemon_review_uses_full_state_goal_when_available` and confirm RED.
2. In `crates/superstate-core/src/daemon.rs`:
   Update line 789 where `superstate_review::execute_review` is called:
   ```rust
   let review_instruction = state.goal.as_deref().unwrap_or(&active_task.title);
   if let Ok(report) = superstate_review::execute_review(
       &working_dir,
       &diff,
       Some(review_instruction),
       &review_cfg,
   ) {
   ```
3. Run `cargo test -p superstate-core --test test_daemon_blocking_review` and confirm GREEN (all 6 tests pass).
4. Run `cargo clippy --all-targets -- -D warnings` and confirm 0 warnings.
5. Commit with conventional commit `feat(daemon): forward full state goal to adversarial code review` with `--no-gpg-sign`.
6. Write summary report to `docs/superpowers/plans/task-1-review-goal-report.md`.
