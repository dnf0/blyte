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

- [ ] **Step 1: Write failing test for `--no-worktree` in prompt generation**
  Add a test verifying that `superstate state prompt --no-worktree` does not provision a `.wt/` directory and omits the worktree prompt section.

- [ ] **Step 2: Add `no_worktree` field to `ExecutionState`**
  In `crates/superstate-core/src/models.rs`:
  Add `#[serde(default)] pub no_worktree: bool` to `ExecutionState`.

- [ ] **Step 3: Update `crates/superstate-cli/src/state_cli.rs` and `crates/superstate-cli/src/commands/state.rs`**
  Add `--no-worktree` flag to `StateCommands::Prompt` and `StateCommands::Init`.
  In `cmd_state_prompt`, if `no_worktree` is true or `state.no_worktree` is true:
  - Skip worktree creation.
  - Set `state.active_worktree = None` and `state.active_branch = None`.
  - Render prompt without worktree paths.
  - Render instruction: `"3. When the verification command passes, stop immediately."` without demanding worktree git commits.

- [ ] **Step 4: Build release binary and verify unit tests**
  Run `cargo test -p superstate-core -p superstate-cli`
  Build `cargo build --release`

- [ ] **Step 5: Commit changes**
  Commit: `feat(cli): add --no-worktree in-place execution mode`

---

### Task 2: Eval Harness Integration for In-Place Rust Execution

- **Depends on:** Task 1
**Files:**
- Modify: `lab/engine/adapters/superstate.py`
- Test: `tests/test_adapters.py`

**Interfaces:**
- Produces: Support for running `superstate-rust` with `--no-worktree` via adapter flag or arm configuration.

- [ ] **Step 1: Update `SuperstateAdapter` to support in-place mode**
  In `lab/engine/adapters/superstate.py`:
  Add `no_worktree: bool = False` configuration to `SuperstateAdapter`.
  When `no_worktree` is true, pass `--no-worktree` to `superstate state init` and `superstate state prompt`.
  Ensure `TARGETED_EXECUTION_CONTRACT` does not mandate worktree commits when in in-place mode.

- [ ] **Step 2: Run adapter unit tests**
  Run `pytest tests/test_adapters.py -v`

- [ ] **Step 3: Commit changes**
  Commit: `feat(adapter): support in-place execution mode for superstate-rust`

---

### Task 3: Benchmark Round 1 — In-Place Rust vs Python Baseline

- **Depends on:** Task 2
**Files:**
- Execute: `scripts/paired_run.py`
- Output: `results/git_leak_recovery_inplace_round1.json`

- [ ] **Step 1: Execute evaluation run in background**
  Run:
  `uv run python scripts/paired_run.py --tasks git-leak-recovery --arms superstate_rust --model gemini-3.8-flash-high --out results/git_leak_recovery_inplace_round1.json`

- [ ] **Step 2: Monitor and record telemetry**
  Capture wall clock, input/output tokens, per-turn duration, pass rate.

- [ ] **Step 3: Compare against Python baseline (240s, 229k tokens, $0.045)**

---

### Task 4: Transparent Orchestrator-Managed Worktrees (Approach 2)

- **Depends on:** Task 3
**Files:**
- Modify: `crates/superstate-cli/src/commands/state.rs`
- Modify: `crates/superstate-worktree/src/manager.rs`

**Interfaces:**
- Produces: Transparent worktree mode where the agent sees only a clean working directory without git instructions, and the orchestrator auto-commits on gate passage.

- [ ] **Step 1: Auto-commit on worktree advance**
  In `superstate state next`: if an active worktree has uncommitted modifications when verification passes, automatically commit them (`git add -A && git commit -m "feat(<task>): implement milestone"`).

- [ ] **Step 2: Clean turnkey prompt**
  Omit git commit requirements from the subagent prompt while retaining worktree filesystem isolation.

- [ ] **Step 3: Benchmark Round 2 and compare telemetry**
