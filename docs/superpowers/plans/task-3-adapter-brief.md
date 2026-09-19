# Task 3 Brief: Adapter Wiring & Parity in `superstate-eval`

## Workspace & Context
- Repo: `/Users/danielfisher/repos/superstate-eval`
- Python environment: `.venv/bin/python`

## What to Implement & Verify
Follow TDD strictly:
1. In `tests/test_superstate_daemon_adapter.py`:
   Write test `test_daemon_spawn_cmd_review_blocking_flags`:
   - Initialize `SuperstateDaemonAdapter(advisory_review=True, review_blocking=True, auto_rollback=False, name="superstate_daemon_review")`.
   - Call `_build_daemon_spawn_cmd(Path("/fake/workspace"))`.
   - Assert `"--advisory-review"` in cmd.
   - Assert `"--review-blocking"` in cmd.
   - Assert `"--no-auto-rollback"` in cmd.
   - Assert `adapter.review_blocking is True`.
   - Assert `adapter.auto_rollback is False`.
2. In `lab/engine/adapters/superstate_daemon.py`:
   Ensure `SuperstateDaemonAdapter.__init__`:
   ```python
   if review_blocking is not None:
       self.review_blocking = review_blocking
   else:
       self.review_blocking = self.advisory_review and review_blocking_enabled()
   ```
   Ensure `_build_daemon_spawn_cmd` appends `--review-blocking` if `self.review_blocking` is True.
3. In `lab/engine/runner.py`:
   Ensure `superstate_daemon_review` resolves to:
   ```python
   elif arm in ("superstate_daemon_review", "superstate-daemon-review", "daemon_review"):
       return SuperstateDaemonAdapter(
           advisory_review=True, review_blocking=True, auto_rollback=False, name="superstate_daemon_review"
       )
   ```
4. In `tests/test_arm_input_parity.py`:
   Ensure `superstate_daemon_review` is included in arm name assertions and passes input parity checks.
5. Run tests and linter:
   - `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py tests/test_arm_input_parity.py -q`
   - `.venv/bin/ruff check lab tests`
6. Commit with conventional commit `feat(eval): wire superstate_daemon_review with full review blocking and edit preservation` (skip GPG with `--no-gpg-sign`).
7. Write summary report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-3-adapter-report.md`.
