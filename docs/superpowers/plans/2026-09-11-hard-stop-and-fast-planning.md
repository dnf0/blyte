# Hard-Stop Execution and Fast Planning Implementation Plan

## Problem
In our telemetry analysis of `db-wal-recovery`:
1. **Model Second-Guessing Friction:** In Turn 2, the subagent successfully fixed and verified the issue at second 50, but because SQLite unlinked `main.db-wal` on checkpoint, the subagent spent the next 186 seconds (37 tool calls) second-guessing the passed verification command, pulling external Docker containers, and searching `/` for eval tests.
2. **Planner Preamble Overhead:** In Turn 1, the model generated 4,400 tokens of conversational explanation and summary before/after writing `plan.md`, adding 33 seconds of planning latency.

## Proposed Changes
1. **Superstate CLI (`crates/superstate-cli`)**:
   - In `cmd_prompt`, replace the legacy TDD instruction with:
     - Minimal direct fix directive.
     - Explicit **HARD STOP UPON PASSING**: "When the verification command passes with exit code 0, finish your turn immediately. Do NOT run extra tests, alternative scripts, or exploratory commands."
2. **Eval Adapter (`superstate-eval`)**:
   - Elevate **HARD STOP UPON PASSING** to Rule 1 in both `TARGETED_EXECUTION_CONTRACT` and `TARGETED_EXECUTION_CONTRACT_INPLACE`.
   - Instruct the planner in `planning_prompt` to output `plan.md` directly via `write_to_file` with zero conversational preamble.
3. **Multi-Task Benchmark Execution**:
   - Run paired comparisons (`superstate_rust_inplace` vs `vanilla`) across `db-wal-recovery`, `openssl-selfsigned-cert`, and `git-leak-recovery`.
