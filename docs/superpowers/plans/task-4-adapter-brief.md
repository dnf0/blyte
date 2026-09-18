# Task 4 Brief: Eval Adapter Wiring & Auto-Rollback Forwarding in `superstate-eval`

## Context & Purpose
Working directory: `/Users/danielfisher/repos/superstate-eval`
Branch: `feat/rapid-canary-eval-and-rollback`

Now that `superstate` daemon supports `--auto-rollback`, `superstate-eval` must be able to configure and pass `--auto-rollback` to the daemon process.

## Requirements
1. **Adapter Update** (`lab/engine/adapters/superstate_daemon.py`):
   - Add constant `AUTO_ROLLBACK_ENV_FLAG = "SUPERSTATE_EVAL_AUTO_ROLLBACK"`.
   - Add helper function `auto_rollback_enabled() -> bool`:
     Returns `True` if `os.environ.get(AUTO_ROLLBACK_ENV_FLAG, "").strip().lower() in ("1", "true", "yes")`.
   - In `SuperstateDaemonAdapter.__init__`:
     - Add parameter `auto_rollback: Optional[bool] = None`.
     - Set `self.auto_rollback: bool = auto_rollback if auto_rollback is not None else auto_rollback_enabled()`.
   - In `_spawn_daemon`:
     - When `self.auto_rollback` is True, append `"--auto-rollback"` to the daemon spawn command.
   - In `get_provenance()`:
     - Record `"auto_rollback": self.auto_rollback`.
2. **Unit Tests** (`tests/test_superstate_daemon_adapter.py`):
   - Add test `test_daemon_adapter_passes_auto_rollback_flag`:
     Instantiates `SuperstateDaemonAdapter(driver=MockDriver(), auto_rollback=True)` and asserts `"--auto-rollback"` is in `_build_daemon_spawn_cmd` output.
   - Add test `test_daemon_adapter_can_disable_auto_rollback`:
     Instantiates with `auto_rollback=False` and asserts `"--auto-rollback"` is not in `_build_daemon_spawn_cmd` output.
   - Test environment variable toggle via `monkeypatch`.
3. **Verification**:
   - `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py` passes cleanly.
   - `.venv/bin/ruff check lab tests` passes with zero errors.
4. **Commit**:
   - Commit with message `feat(eval): wire auto_rollback into superstate daemon adapter` (use `--no-gpg-sign`).
5. **Report**:
   - Write full report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-4-adapter-report.md`.
