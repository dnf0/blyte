# Task 4 Report: Eval Adapter Wiring & Auto-Rollback Forwarding in `superstate-eval`

**Status:** Completed  
**Commit SHA:** `c2741fb` (`c2741fbdc07769c63f355bd0765194a058e134a5`)  
**Branch:** `feat/rapid-canary-eval-and-rollback`  
**Repository:** `/Users/danielfisher/repos/superstate-eval`  
**Date:** 2026-09-18  

---

## Executive Summary

Task 4 integrated the daemon `--auto-rollback` feature into the evaluation harness in `superstate-eval`, as specified in `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-4-adapter-brief.md`.

The evaluation adapter `SuperstateDaemonAdapter` now supports configurable auto-rollback behavior:
1. **Environment Variable Configuration**: `AUTO_ROLLBACK_ENV_FLAG = "SUPERSTATE_EVAL_AUTO_ROLLBACK"`, with helper `auto_rollback_enabled() -> bool` returning `True` for truthy values (`"1"`, `"true"`, `"yes"`).
2. **Explicit Constructor Parameter**: `SuperstateDaemonAdapter.__init__` accepts `auto_rollback: Optional[bool] = None` (falling back to `auto_rollback_enabled()`).
3. **Daemon Invocation Forwarding**: `_build_daemon_spawn_cmd` and `_spawn_daemon` append `"--auto-rollback"` to the spawned detached daemon command line when `self.auto_rollback` is `True`.
4. **Execution Provenance**: `get_provenance()` records `"auto_rollback": self.auto_rollback` in the evaluation run metadata.

---

## TDD Execution

### 1. Red Phase
Added comprehensive unit tests to `tests/test_superstate_daemon_adapter.py`:
- `MockDriver`: Mock agent driver implementing `AgentDriver` interface for adapter tests.
- `test_daemon_adapter_passes_auto_rollback_flag`: Instantiates `SuperstateDaemonAdapter(driver=MockDriver(), auto_rollback=True)` and verifies `"--auto-rollback"` is present in `_build_daemon_spawn_cmd()` output, and recorded in provenance.
- `test_daemon_adapter_can_disable_auto_rollback`: Instantiates `SuperstateDaemonAdapter(driver=MockDriver(), auto_rollback=False)` and verifies `"--auto-rollback"` is omitted from `_build_daemon_spawn_cmd()` output, and recorded as `False` in provenance.
- `test_daemon_adapter_auto_rollback_env_var`: Verifies environment variable toggle via `monkeypatch` across unset, truthy values (`"1"`, `"true"`, `"yes"`, case-insensitively), and falsy values.
- `test_daemon_adapter_spawn_daemon_forwards_auto_rollback`: Verifies that `_spawn_daemon` passes `"--auto-rollback"` to `subprocess.run`.

Ran `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py -k auto_rollback` and verified expected RED failure:
```text
ImportError while importing test module '/Users/danielfisher/repos/superstate-eval/tests/test_superstate_daemon_adapter.py'.
E   ImportError: cannot import name 'AUTO_ROLLBACK_ENV_FLAG' from 'lab.engine.adapters.superstate_daemon' (/Users/danielfisher/repos/superstate-eval/lab/engine/adapters/superstate_daemon.py)
```

### 2. Green Phase
Implemented changes in `lab/engine/adapters/superstate_daemon.py`:
- Defined `AUTO_ROLLBACK_ENV_FLAG = "SUPERSTATE_EVAL_AUTO_ROLLBACK"`.
- Defined helper function `auto_rollback_enabled() -> bool`:
  ```python
  def auto_rollback_enabled() -> bool:
      """Whether automatic rollback on milestone failure is enabled for the daemon."""
      return os.environ.get(AUTO_ROLLBACK_ENV_FLAG, "").strip().lower() in ("1", "true", "yes")
  ```
- Updated `SuperstateDaemonAdapter.__init__`:
  - Added parameters `auto_rollback: Optional[bool] = None` and `driver: Any = None`.
  - Set `self.auto_rollback: bool = auto_rollback if auto_rollback is not None else auto_rollback_enabled()`.
  - Stored `self.driver = driver`.
- Added `_build_daemon_spawn_cmd(self, workspace_path: Optional[Path] = None) -> list[str]`:
  - Builds the daemon command argument list.
  - Appends `"--auto-rollback"` when `self.auto_rollback` is `True`.
- Updated `_spawn_daemon(self, workspace_path: Path) -> bool`:
  - Delegates command construction to `_build_daemon_spawn_cmd(workspace_path)`.
- Updated `get_provenance()`:
  - Added `"auto_rollback": self.auto_rollback` to the metadata dictionary.

### 3. Verification & Linter Checks
1. **Targeted Adapter Tests**:
   `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py` -> **30 passed; 0 failed (100% clean)**.
2. **Linter Check**:
   `.venv/bin/ruff check lab tests` -> **All checks passed (0 errors)**.
3. **Full Evaluation Test Suite**:
   `.venv/bin/python -m pytest tests/ -q` -> **328 passed in 61.73s (100% clean)**.

---

## Git Commit Details

- **Commit SHA:** `c2741fb` (`c2741fbdc07769c63f355bd0765194a058e134a5`)
- **Message:** `feat(eval): wire auto_rollback into superstate daemon adapter` (with `--no-gpg-sign`)
- **Files Modified:**
  - `lab/engine/adapters/superstate_daemon.py`
  - `tests/test_superstate_daemon_adapter.py`
