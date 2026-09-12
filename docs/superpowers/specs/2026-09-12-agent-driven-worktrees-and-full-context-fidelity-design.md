# Design Specification: Agent-Driven Worktrees & Full-Context Turnkey Execution

- **Date:** 2026-09-12
- **Topic:** Agent-Driven Worktree Isolation and Full-Context Turnkey Execution
- **Status:** Proposed
- **Authors:** Superstate Engineering Team

---

## 1. Problem Statement & Motivation

In recent paired benchmark evaluations against Vanilla (unguided baseline) and Superpowers, Superstate demonstrated architectural dominance on complex, multi-file software engineering tasks (e.g., 100% pass on `anko`, 50% vs 0% on `httpx`, and 95%+ partial test score on `meriyah`, `expr`, and `katex`). However, Superstate lagged in binary pass rate on two specific fronts:

1. **Terminal-Bench Tasks (`openssl-selfsigned-cert`, `build-cython-ext`):**
   - Superstate scored **0/2** while Vanilla and Superpowers scored **2/2**, despite Superstate writing the correct functional code (passing 5/5 functional tests in openssl and 10/11 in cython).
   - **Root Cause:** Superstate unconditionally forced git worktree isolation (`.wt/task-1`) on every task. Git tree objects only track `0644` vs `0755` permissions, silently resetting `chmod 600 key.pem` back to `0644` upon worktree merge. Similarly, untracked sub-repositories (`git clone pyknotid`) were ignored by Git commits and destroyed when the worktree was cleaned up.
2. **SWE-Bench Edge Cases (`meriyah`, `expr`, `katex`):**
   - Superstate passed **45/49** held-out tests on `meriyah`, **77/79** on `expr`, and **89/94** on `katex`.
   - **Root Cause A (Context Loss):** `render_turnkey_prompt` only provided the subagent with the 1-line milestone title (e.g. `Task 1: Add using keyword to lexer`), stripping out the 20 lines of nuanced user requirements, edge cases, formatting rules, and flags.
   - **Root Cause B (Blind Planning Turn):** Because SWE tasks lacked pre-declared `verification_cmd`s (due to sealed test suites), Superstate spent an entire blind planning turn guessing file structures and abstract milestones without shell or file access, incurring a $0.15–$0.25 cost penalty and creating disconnected milestones.

---

## 2. Core Architectural Principles

1. **Worktrees are an Agent-Driven Capability, Not an Unconditional Wrapper:**
   - The state machine (`superstate state`) must default to in-place execution in the repository root.
   - Worktree isolation must be an on-demand primitive (`superstate worktree create` / `superstate worktree merge`, or `--worktree` flag) that the agent or user invokes when branch isolation or clean rollback is genuinely beneficial.
   - Non-repo directories, simple CLI scripts, and bash puzzles run directly in-place with 100% native POSIX filesystem fidelity.
2. **Full Context Fidelity Everywhere:**
   - The subagent must never lose the global problem specification. `ExecutionState` records the full task goal/instruction, and `render_turnkey_prompt` includes the complete `## Task Objective & Requirements` in every turn.
3. **Zero-Turn Direct Execution (Eliminate Blind Planning):**
   - Phase 1 blind planning is eliminated. All tasks initialize directly with the full task goal. The agent begins execution in Turn 1 with immediate shell and filesystem access, matching Vanilla's single-pass efficiency.

---

## 3. Detailed Component Architecture

### Component 1: `superstate-core` (State Models & Turnkey Prompt)

#### 1.1 `ExecutionState` Model (`crates/superstate-core/src/models.rs`)
- Add `pub goal: Option<String>` to `ExecutionState`.
- `init_state_direct` and `init_state_milestones` store the user's high-level goal in `state.goal`.
- When serializing/deserializing `state.json`, `goal` is preserved.

#### 1.2 Full-Context Turnkey Prompt (`crates/superstate-core/src/prompt.rs`)
- Update `render_turnkey_prompt`:
  - If `state.goal` is present, render:
    ```markdown
    ## 🎯 Global Task Objective & Requirements
    <state.goal>
    ```
  - Followed by the active milestone specification, active worktree details (if any), completed milestones, and recent epistemic ledger notes.
  - The implementer subagent now has complete visibility into all requirements, edge cases, and invariants.

---

### Component 2: `superstate-cli` & `superstate-worktree` (Agent-Driven Isolation)

#### 2.1 Default In-Place Execution in `state init`
- In `crates/superstate-cli/src/commands/state.rs`:
  - `superstate state init` defaults to in-place execution (`no_worktree: true`) unless the caller explicitly passes `--worktree`.
  - In-place execution becomes the reliable, zero-friction default for general tasks.

#### 2.2 Dedicated Agent Worktree Commands
- `superstate worktree create <task-id>`:
  - Validates that the current directory is a Git repository.
  - Provisions an isolated worktree under `.wt/<branch-or-task>`, links COW caches (`target/`, `.venv/`, `node_modules/`), and updates `state.active_worktree` in `state.json`.
- `superstate worktree merge`:
  - Auto-commits changes in the active worktree, merges into the base branch, and tears down the worktree.
  - If the agent ran in-place without a worktree, `superstate state next` simply advances the milestone in the current workspace.

---

### Component 3: `superstate-eval` Harness & Adapter (`lab/engine/adapters/superstate.py`)

#### 3.1 Universal Zero-Turn Direct Initialization
- Update `SuperstateAdapter.execute`:
  - Remove the requirement that `vcmd` must be non-empty for `zero_turn_planning`.
  - For tasks without a pre-declared verification command (e.g. SWE-bench tasks where tests are sealed), initialize state directly with:
    `superstate state init --goal "<task.instruction>"` (which runs in-place by default).
  - Eliminate the blind planning agy turn (Phase 1).
  - The agent enters Turn 1 immediately with the full task instruction and repository access.

#### 3.2 Dynamic Turn & Milestone Execution
- The agent executes Turn 1. It can:
  - Work directly in-place or optionally invoke `superstate worktree create`.
  - Record findings and hypotheses via `superstate state note`.
  - Run tests and self-verify edge cases before signaling completion.

---

## 4. Error Handling & Edge Cases

1. **Non-Git Workspaces (e.g., Terminal-Bench):**
   - Because default execution is in-place, `superstate state init` succeeds even if `.git` does not exist.
   - If an agent attempts to run `superstate worktree create` in a non-git directory, it returns a clean, actionable error: `"Cannot create worktree: workspace is not a Git repository."`
2. **POSIX File Permissions (`chmod 600`):**
   - In in-place execution, file permissions on disk are preserved directly by the filesystem. No Git checkout clobbers `0600` or `0400` bits.
3. **Submodules and Cloned Sub-Repositories:**
   - In in-place execution, cloned repositories (e.g. `pyknotid`) exist directly at `/app/pyknotid` and are never lost to worktree cleanup.

---

## 5. Verification & Test Plan

1. **Unit & Integration Tests in `superstate`:**
   - Verify `ExecutionState` serde roundtrip with `goal: Some(...)`.
   - Verify `render_turnkey_prompt` includes `## 🎯 Global Task Objective & Requirements`.
   - Verify `superstate state init` defaults to in-place unless `--worktree` is passed.
2. **Unit Tests in `superstate-eval`:**
   - Verify `SuperstateAdapter` initializes directly without planning turn for tasks with `verification_cmd=None`.
3. **Targeted Benchmark Validation (Subset):**
   - **Terminal-Bench Subset:** Run paired evaluation on `openssl-selfsigned-cert` and `build-cython-ext`. Verify Superstate passes 2/2 on both (turning 0/2 into 2/2).
   - **SWE-Bench Subset:** Run paired evaluation on `meriyah-explicit-resource-declarations`, `expr-try-catch-errors`, and `katex-multicolumn-array-spans`. Verify full prompt context and Turn 1 execution closes the edge-case gap.
