# Task 1 Brief: Canary 3 Task Set & Evaluation Runner in `superstate-eval`

## Context & Purpose
Working directory: `/Users/danielfisher/repos/superstate-eval`
Branch: `feat/rapid-canary-eval-and-rollback`

We need a dedicated, fast 3-task Canary slice that finishes in 12–15 minutes instead of 6–12 hours, allowing rapid iteration on state management features.

## Requirements
1. **Task Set Definitions** (`lab/benchmarks/task_sets.py`):
   - Define `CANARY_TASKS: tuple[str, ...]` with:
     - `"build-cython-ext"`
     - `"chess-best-move"`
     - `"httpx-multipart-response-parsing"`
   - Add function `is_canary(task_name: str) -> bool`.
2. **CLI Runner Flag** (`scripts/paired_run.py`):
   - Add `--canary` flag (`action="store_true"`, help: `"Run the fast 3-task Canary slice (~15 min total)"`).
   - When `--canary` is supplied, select `CANARY_TASKS` as the tasks to evaluate (similar to `--core` selecting `DISCRIMINATING_TASKS`).
3. **Turnkey Shell Script** (`scripts/run_canary.sh`):
   - Create executable bash script `scripts/run_canary.sh`:
     - Default arms: `superstate_daemon_review superstate_mcp` (or `$1` if provided).
     - Timestamped output file in `results/canary_<stamp>.json`.
     - Invokes `scripts/paired_run.py` with `--canary`, `--driver agy`, `--model gemini-3.8-flash-high`, `--concurrency 2`.
     - chmod +x.
4. **TDD Unit Tests** (`tests/test_canary_task_set.py`):
   - Write tests verifying `CANARY_TASKS`, `is_canary`, and that every canary task is in `DISCRIMINATING_TASKS`.
   - Run via `.venv/bin/python -m pytest tests/test_canary_task_set.py`.
5. **Linting & Formatting**:
   - Ensure `.venv/bin/ruff check lab scripts tests` passes cleanly.
6. **Commit**:
   - Conventional commit: `feat(eval): add canary 3 task set and runner script` (skip GPG verification with `--no-gpg-sign`).
7. **Report**:
   - Write full report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-1-canary-report.md`.
