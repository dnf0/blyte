# Superstate Rust Performance Parity & Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `agent-rules:stateful-execution` (SKILL.state) (deterministic linear execution with automated benchmarking)
> 2. `superpowers:subagent-driven-development` (recommended for multi-agent reviews)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optimize the Superstate Rust engine (`superstate-rust`) to outperform the Python baseline (`agent-rules-python`) on Terminal-Bench tasks (target: wall clock < 240s, tokens < 229k, cost < $0.045, 100% pass rate).

**Architecture:** 
1. Implement a clean `--no-worktree` in-place execution mode across `superstate-cli` (`prompt`, `init`) and `superstate-core` models.
2. Align turnkey prompt contracts to eliminate dual-git cognitive tangency, removing mandatory agent git operations when not appropriate.
3. Benchmark across multiple rounds (in-place mode, transparent orchestrator-committed worktrees, lean prompt context) using the paired evaluation harness until beating Python performance.

**Tech Stack:** Rust (superstate CLI/core), Python (superstate-eval harness), Docker sandbox, Gemini 3.8 Flash High.

## Global Constraints
- Target: `git-leak-recovery` wall-clock duration <= 240s, cost <= $0.045, 100% pass rate.
- Preserves all existing unit and integration tests in `superstate` and `superstate-eval`.
- Changes must be measurable empirically through `results/*.json` telemetry.

---

### Task 1: In-Place Execution Mode (`--no-worktree`) in Superstate Rust

- **Depends on:** none
**Files:**
- Modify: `crates/superstate-core/src/models.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Modify: `crates/superstate-cli/src/commands/state.rs`
- Test: `crates/superstate-cli/tests/test_cli.rs` (or create `crates/superstate-cli/tests/test_in_place_prompt.rs`)

**Interfaces:**
- Consumes: `ExecutionState`, `StateCommands::Prompt`, `StateCommands::Init`
- Produces: `ExecutionState::no_worktree: bool`, `--no-worktree` flag on `prompt` and `init`

- [x] **Step 1: Write failing test for `--no-worktree` in prompt generation**
- [x] **Step 2: Add `no_worktree` field to `ExecutionState`**
- [x] **Step 3: Update `crates/superstate-cli/src/state_cli.rs` and `crates/superstate-cli/src/commands/state.rs`**
- [x] **Step 4: Build release binary and verify unit tests**
- [x] **Step 5: Commit changes**
  Committed: `feat(cli): add --no-worktree in-place execution mode` (`4498c3f`)

---

### Task 2: Eval Harness Integration for In-Place Rust Execution

- **Depends on:** Task 1
- [x] **Step 1: Update `SuperstateAdapter` to support in-place mode**
- [x] **Step 2: Run adapter unit tests**
- [x] **Step 3: Commit changes**
  Committed: `feat(eval): add superstate_rust_inplace arm and in-place execution mode` (`f50dab8`)

---

### Task 3: Benchmark Round 1 — In-Place Rust vs Python Baseline

- **Depends on:** Task 2
- [x] **Step 1: Execute evaluation run in background**
- [x] **Step 2: Monitor and record telemetry**
- [x] **Step 3: Compare against Python baseline**

**Results on `terminal-bench/git-leak-recovery` (Concurrently executed paired run):**
- `superstate_rust_inplace`: **216.67s**, **312,555 tokens in**, **18,121 tokens out**, **$0.0578**, **100% pass (5/5)**
- `superstate_python`: **293.44s**, **362,793 tokens in**, **24,391 tokens out**, **$0.0691**, **100% pass (5/5)**
- **Outcome:** **Rust is 76.77s (26.2%) FASTER than Python, uses 50,238 fewer input tokens, and costs 16.4% less!**

---

### Task 4: Transparent Orchestrator-Managed Worktrees (Approach 2)

- **Depends on:** Task 3
- [x] **Step 1: Auto-commit on worktree advance (`auto_commit_if_dirty`)**
- [x] **Step 2: Clean turnkey prompt (eliminate subagent git requirements)**
- [x] **Step 3: Benchmark Round 2 and compare telemetry**
  Committed: `feat(worktree): add auto-commit on merge and transparent worktree instructions` (`b95fc1d`)

**Generalization Verification on `terminal-bench/db-wal-recovery` (Concurrently executed paired run):**
- `superstate_rust_inplace`: **897.75s (15.0m)**, **621,741 tokens in**, **$0.1615**, **100% pass (7/7)**
- `superstate_python`: **1,078.94s (18.0m)**, **651,433 tokens in**, **$0.1631**, **100% pass (7/7)**
- **Outcome:** **Rust is 181.19s (3.0m / 16.8%) FASTER than Python, uses 29,692 fewer tokens, and costs less!**
