# Task 3 Brief: Arm Input Parity Verification & Container Execution Check

## Context & Objective
In `repos/superstate-eval`, verify that:
1. Arm input parity is 100% preserved (strictly zero prompt engineering).
2. The newly compiled release binary `superstate` supports `inspect` both on the host and inside docker containers.
3. All unit and integration tests in `superstate-eval` pass cleanly.

## Working Directory
- Repo: `/Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd`
- Branch: `feat/eval-philosophy-autonomous-tdd`

## Execution Steps
1. Use `.venv/bin/python` for all commands.
2. Run `.venv/bin/python -m pytest tests/test_arm_input_parity.py -v`.
   - Verify all parity checks PASS.
3. Test `superstate inspect` live on host:
   - Run `~/.cargo/bin/superstate inspect lab/benchmarks/task_loader.py --lines 1:15`.
   - Run `~/.cargo/bin/superstate inspect lab/benchmarks/task_loader.py --outline`.
   - Run `~/.cargo/bin/superstate inspect lab/benchmarks/task_loader.py --symbol TaskSpec`.
4. Run full test suite:
   - Run `.venv/bin/python -m pytest tests/ -q`.
   - Ensure all 343+ tests pass.
5. Write detailed report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-3-parity-report.md`.
