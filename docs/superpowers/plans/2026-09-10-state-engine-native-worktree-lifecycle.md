# State Engine Native Worktree Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate Git worktree lifecycles natively into Superstate's state engine (`superstate state`) and evaluation harness (`superstate-eval`), making isolated worktrees the default execution environment for all milestones.

**Architecture:** Update `ExecutionState` to track active worktrees, extend `superstate-worktree` with milestone lifecycle methods (`ensure_task_worktree`, `merge_and_cleanup`), update `superstate state prompt` to auto-provision worktrees and inject directory contracts, update `superstate state next` to execute verification gates in the worktree and merge/cleanup on success, and update `superstate-eval` to execute subagents inside the isolated worktree checkout.

**Tech Stack:** Rust (edition 2021, `worktrunk`, `git2`, `serde`, `clap`), Python 3.12 (`pytest`, `docker`).

## Global Constraints
- Target repositories: `/Users/danielfisher/repos/superstate` and `/Users/danielfisher/repos/superstate-eval`.
- No unhandled unwraps in library code; return explicit `Result<T, WorktreeError>` or `Result<T, String>`.
- Preserved worktree isolation: `.wt/` must be ignored from primary git status.
- Strict backward compatibility: if git is unavailable, fall back cleanly to in-place execution.
- Tests written first (TDD) for every task before implementation.

---

### Task 1: Add Worktree Tracking Fields to `ExecutionState`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/models.rs`
- Modify: `crates/superstate-core/src/state_machine.rs`
- Test: `crates/superstate-core/tests/test_state_machine.rs`

**Interfaces:**
- Produces: `ExecutionState::active_worktree: Option<PathBuf>`, `ExecutionState::active_branch: Option<String>`

- [ ] **Step 1: Write the failing test**
Create a test in `crates/superstate-core/tests/test_state_machine.rs` asserting that `ExecutionState` serializes, deserializes, and resets `active_worktree` and `active_branch` upon step advancement.

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test --test test_state_machine`
Expected: Compile failure (fields `active_worktree` and `active_branch` do not exist).

- [ ] **Step 3: Implement minimal code**
Add `active_worktree: Option<PathBuf>` and `active_branch: Option<String>` with `#[serde(default)]` to `ExecutionState` in `crates/superstate-core/src/models.rs`.
In `crates/superstate-core/src/state_machine.rs`, update `advance_step` to clear `active_worktree = None` and `active_branch = None` when transitioning to the next task.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --test test_state_machine`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/superstate-core
git commit -m "feat(core): add active_worktree and active_branch tracking to ExecutionState"
```

---

### Task 2: Implement Milestone Worktree Lifecycle Helpers in `superstate-worktree`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-worktree/src/manager.rs`
- Test: `crates/superstate-worktree/tests/test_worktree_lifecycle.rs`

**Interfaces:**
- Consumes: `WorktreeManager`, `IsolatedWorktree`
- Produces:
  - `WorktreeManager::ensure_task_worktree(&self, task_slug: &str, base_branch: Option<&str>) -> Result<IsolatedWorktree, WorktreeError>`
  - `IsolatedWorktree::merge_and_cleanup(self, commit_msg: &str) -> Result<(), WorktreeError>`

- [ ] **Step 1: Write the failing test**
In `crates/superstate-worktree/tests/test_worktree_lifecycle.rs`, add a test `test_ensure_task_worktree_and_merge_cleanup` that creates a test git repository, calls `ensure_task_worktree`, writes a commit inside the worktree, and calls `merge_and_cleanup`, verifying changes appear on `main` and the worktree is deleted.

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-worktree --test test_worktree_lifecycle`
Expected: Compile failure (methods not found on `WorktreeManager` / `IsolatedWorktree`).

- [ ] **Step 3: Implement minimal code**
In `crates/superstate-worktree/src/manager.rs`:
- Implement `ensure_task_worktree(&self, task_slug: &str, base_branch: Option<&str>) -> Result<IsolatedWorktree, WorktreeError>` which creates `.wt/task-<slug>` if absent, using branch `superstate/task-<slug>` and reflink cache dirs.
- Implement `merge_and_cleanup(self, commit_msg: &str) -> Result<(), WorktreeError>` which calls `self.merge_into_base(commit_msg)` followed by `self.destroy(false)`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-worktree --test test_worktree_lifecycle`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/superstate-worktree
git commit -m "feat(worktree): add milestone lifecycle methods to WorktreeManager"
```

---

### Task 3: Integrate Worktree Provisioning and Prompt Contract in `superstate-cli`

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-cli/src/commands/state.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Test: `crates/superstate-cli/tests/test_state_prompt_worktree.rs`

**Interfaces:**
- Consumes: `WorktreeManager::ensure_task_worktree`
- Produces: Worktree provisioning during `cmd_prompt`, injecting worktree contract into turnkey prompt, adding `--json` to `StateCommands::Prompt`.

- [ ] **Step 1: Write the failing test**
In `crates/superstate-cli/tests/test_state_prompt_worktree.rs`, create a test initializing state in a git repository and calling `cmd_prompt`. Assert that `.wt/task-1` exists, `state.json` contains `active_worktree`, and prompt includes the worktree path.

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate --test test_state_prompt_worktree`
Expected: FAIL.

- [ ] **Step 3: Implement minimal code**
In `crates/superstate-cli/src/commands/state.rs`:
- Update `cmd_prompt`: check if git repository is present. If so, initialize `WorktreeManager`, call `ensure_task_worktree`, update `state.active_worktree` and `state.active_branch`, and save `state.json`.
- Append worktree contract section to the prompt output:
  `## 🌲 Isolated Worktree Environment`
  `Working Directory: <path>`
- Support `--json` flag to emit JSON representation `{ "active_worktree": "...", "prompt": "..." }`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate --test test_state_prompt_worktree`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/superstate-cli
git commit -m "feat(cli): provision worktree and inject contract during state prompt"
```

---

### Task 4: Integrate Gate Verification in Worktree, Atomic Merge, and Cleanup in `state next`

- **Depends on:** Task 3

**Files:**
- Modify: `crates/superstate-cli/src/commands/state.rs`
- Test: `crates/superstate-cli/tests/test_state_worktree_lifecycle.rs`

**Interfaces:**
- Consumes: `IsolatedWorktree::merge_and_cleanup`, `execute_all_gates`
- Produces: `cmd_next` running gates inside worktree, merging on success, and preserving worktree on failure.

- [ ] **Step 1: Write the failing test**
In `crates/superstate-cli/tests/test_state_worktree_lifecycle.rs`, create a complete end-to-end multi-milestone test:
1. `cmd_prompt`: provisions `.wt/task-1`.
2. Commit changes inside `.wt/task-1`.
3. `cmd_next --verify-git-clean`: asserts that gates run inside `.wt/task-1`, worktree is merged to `main`, `.wt/task-1` is deleted, and state advances to Task 2.
4. Second test: verify that a failing gate does NOT merge, preserves `.wt/task-1`, and records failure in `state.errors`.

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate --test test_state_worktree_lifecycle`
Expected: FAIL.

- [ ] **Step 3: Implement minimal code**
In `crates/superstate-cli/src/commands/state.rs`:
- In `cmd_next`:
  - Determine execution target: if `state.active_worktree` is Some and directory exists, target is `active_worktree`. Otherwise `repo_root`.
  - Pass target to `execute_all_gates`.
  - If any gate fails: save `state.errors` and return error without deleting worktree.
  - If all gates pass:
    - If `state.active_worktree` was used, instantiate `IsolatedWorktree` and call `merge_and_cleanup`.
    - Clear `state.active_worktree = None` and `state.active_branch = None`.
    - Advance step with `advance_step(&mut state)`.
    - Save atomic `state.json`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate --test test_state_worktree_lifecycle`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/superstate-cli
git commit -m "feat(cli): execute gates in worktree and merge on success in state next"
```

---

### Task 5: Update `superstate-eval` Adapter and Run Live Performance Verification

- **Depends on:** Task 4

**Files:**
- Modify: `lab/engine/adapters/superstate.py` (in `superstate-eval`)
- Test: `tests/test_pilot_runner.py` (in `superstate-eval`)

**Interfaces:**
- Consumes: `superstate state prompt`, `superstate state next`
- Produces: Harness directing worker driver (`agy`) to execute in the active worktree path.

- [ ] **Step 1: Write the failing test**
In `tests/test_pilot_runner.py`, add a test verifying that `SuperstateAdapter` detects active worktrees and routes `driver.run` to the worktree path.

- [ ] **Step 2: Run test to verify it fails**
Run: `uv run pytest tests/test_pilot_runner.py -k worktree`
Expected: FAIL.

- [ ] **Step 3: Implement minimal code**
In `lab/engine/adapters/superstate.py`:
- After calling `turnkey_prompt`, inspect `.agent-state/state.json` for `active_worktree`.
- If `active_worktree` exists, set `turn_workspace = workspace_path / active_worktree` (or relative path).
- Pass `turn_workspace` to `driver.run()`.
- Update `TARGETED_EXECUTION_CONTRACT` to instruct the worker to operate in the active worktree.

- [ ] **Step 4: Run test to verify it passes**
Run: `uv run pytest tests/test_pilot_runner.py`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add lab/engine/adapters/superstate.py tests/test_pilot_runner.py
git commit -m "feat(adapter): route milestone execution into native worktree checkouts"
```

---

## Self-Review Checklist
- [x] All spec requirements covered across Tasks 1–5.
- [x] Exact file paths, signatures, and commands specified.
- [x] No placeholders or vague TODOs.
- [x] Clean TDD flow for each task.
