# Task 1 Report: Canary 3 Task Set & Evaluation Runner in `superstate-eval`

**Status:** Completed  
**Commit SHA:** `c264507`  
**Branch:** `feat/rapid-canary-eval-and-rollback`  
**Repository:** `/Users/danielfisher/repos/superstate-eval`  
**Date:** 2026-09-18  

---

## Executive Summary

Task 1 established a dedicated, rapid Canary 3 evaluation slice and turnkey runner in `superstate-eval` as specified in `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-1-canary-brief.md`.

Full evaluation runs across the benchmark corpus require 6–12 hours, creating significant latency when testing state-management control planes and review mechanics. The Canary 3 task set provides a fast, high-signal 12–15 minute evaluation loop comprising:
1. `build-cython-ext` (terminal-bench, build/compilation & C-extension)
2. `chess-best-move` (terminal-bench, algorithmic logic & domain modeling)
3. `httpx-multipart-response-parsing` (datacurve / tier-1 repository task, complex async HTTP & multipart RFC handling)

All three tasks are strictly **gradeable** (with verified sealed held-out graders at container root) and belong to `DISCRIMINATING_TASKS`, ensuring they are neither unmeasurable nor subject to ceiling/floor plateaus.

---

## TDD Execution

### 1. Red Phase
Created `tests/test_canary_task_set.py` specifying requirements:
- `test_canary_tasks_definition`: Exact 3-tuple definition of `CANARY_TASKS`.
- `test_is_canary`: Correct membership predicate rejecting unrelated or non-existent tasks.
- `test_canary_tasks_are_discriminating`: All canary tasks verified in `DISCRIMINATING_TASKS`.
- `test_canary_tasks_are_measurable`: Verifies no canary task is marked unmeasurable or missing a held-out grader.
- `test_run_canary_script_exists_and_executable`: Verifies `scripts/run_canary.sh` exists and has execute permissions (`+x`).
- `test_paired_run_canary_flag`: Verifies `paired_run.py` accepts and parses `--canary`.

Ran `.venv/bin/python -m pytest tests/test_canary_task_set.py` to verify RED failure:
```text
==================================== ERRORS ====================================
________________ ERROR collecting tests/test_canary_task_set.py ________________
ImportError while importing test module '/Users/danielfisher/repos/superstate-eval/tests/test_canary_task_set.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
../../.local/share/uv/python/cpython-3.12.9-macos-aarch64-none/lib/python3.12/importlib/__init__.py:90: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
tests/test_canary_task_set.py:9: in <module>
    from lab.benchmarks.task_sets import (
E   ImportError: cannot import name 'CANARY_TASKS' from 'lab.benchmarks.task_sets' (/Users/danielfisher/repos/superstate-eval/lab/benchmarks/task_sets.py)
=========================== short test summary info ============================
ERROR tests/test_canary_task_set.py
!!!!!!!!!!!!!!!!!!!! Interrupted: 1 error during collection !!!!!!!!!!!!!!!!!!!!
=============================== 1 error in 0.15s ===============================
```

### 2. Green Phase
- Defined `CANARY_TASKS: tuple[str, ...]` and `is_canary(task_name: str) -> bool` in `lab/benchmarks/task_sets.py`.
- Refactored `scripts/paired_run.py` to expose `build_parser()`, added `--canary` argument (`action="store_true"`), and added task filtering logic:
  ```python
  if args.canary:
      before = len(tasks)
      tasks = [t for t in tasks if is_canary(t.name)]
      logger.info(
          "Canary slice: %d of %d tasks (%s).",
          len(tasks), before, ", ".join(t.name for t in tasks),
      )
  ```
- Created executable script `scripts/run_canary.sh` (`chmod +x`), configured with:
  - Default arms: `superstate_daemon_review superstate_mcp` (or arguments if supplied)
  - Driver: `agy`
  - Model: `gemini-3.8-flash-high`
  - Concurrency: `2` (within governor limits)
  - Output: `results/canary_<stamp>.json`
- Ran `.venv/bin/python -m pytest tests/test_canary_task_set.py`:
  ```text
  tests/test_canary_task_set.py ......                                     [100%]
  ============================== 6 passed in 0.14s ===============================
  ```
- Ran full test suite `.venv/bin/python -m pytest tests/ -q`:
  ```text
  324 passed in 62.48s (0:01:02)
  ```

### 3. Quality & Linting Checks
- Ran `.venv/bin/ruff check lab scripts tests` and resolved existing formatting / unused imports in `scripts/eval_routing_split.py`, `scripts/gate_precision.py`, and `scripts/harvest_run_diffs.py`.
- Verified `.venv/bin/ruff check lab scripts tests` passes cleanly with 0 errors.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `lab/benchmarks/task_sets.py` | Modified | Defined `CANARY_TASKS` tuple and `is_canary(task_name)` predicate |
| `scripts/paired_run.py` | Modified | Exposed `build_parser()`, added `--canary` CLI argument and task selection filter |
| `scripts/run_canary.sh` | Created | Executable turnkey script for Canary evaluations with default arms and concurrency |
| `tests/test_canary_task_set.py` | Created | Added 6 unit tests validating Canary task definitions, discrimination, measurability, runner script, and CLI flag |
| `scripts/eval_routing_split.py` | Modified | Cleaned up unused import and static f-string formatting |
| `scripts/gate_precision.py` | Modified | Cleaned up static f-string formatting |
| `scripts/harvest_run_diffs.py` | Modified | Cleaned up unused `os` import |

---

## Commit Details

- **Commit SHA:** `c2645077bf1c0e9c09d9f8369f6448c333bbd80f` (`c264507`)
- **Message:** `feat(eval): add canary 3 task set and runner script`
- **GPG Verification:** Skipped (`--no-gpg-sign`) per repository guidance.

---

## Verification & Usage

The canary evaluation can now be executed directly with:
```bash
# Default arms: superstate_daemon_review vs superstate_mcp
./scripts/run_canary.sh

# Or with custom arms:
./scripts/run_canary.sh superstate_daemon superstate_mcp
```
Or via the Python runner:
```bash
.venv/bin/python scripts/paired_run.py --canary --arms superstate_daemon_review superstate_mcp --concurrency 2
```
