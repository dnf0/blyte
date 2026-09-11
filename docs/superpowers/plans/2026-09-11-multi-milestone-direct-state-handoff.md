# Multi-Milestone Direct State Handoff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable direct multi-milestone state handoff from chat into Superstate without generating `plan.md` files on disk, ensuring milestone worktrees merge cleanly into the user's active feature branch.

**Architecture:** Extend `superstate-core` with `MilestoneSpec` and `init_state_milestones` to construct typed multi-task DAGs in memory. Update `superstate-worktree` with active feature branch detection (`git rev-parse --abbrev-ref HEAD`). Wire `--milestones` into `superstate-cli` (including stdin `-` support) and `superstate_handoff` into `superstate-mcp` with automatic sequential worktree provisioning and auto-merges into the feature branch.

**Tech Stack:** Rust 1.80+ (workspace crates: `superstate-core`, `superstate-worktree`, `superstate-cli`, `superstate-mcp`), `serde`, `serde_json`, `worktrunk`.

## Global Constraints
- Target branch for all milestone merges MUST be the active feature branch captured at initialization, NEVER `main`.
- In-memory state construction: 0 markdown files (`plan.md`, `plan.normalized.md`) may be created on disk.
- Fail-closed validation: invalid milestones (empty title, missing `verify_cmd`) must abort before writing state.
- Backward compatibility: single-milestone handoff (`goal` + `verify_cmd`) and legacy `--plan` remain 100% functional.
- All code changes must pass `cargo test --all` and `cargo clippy`.

---

### Task 1: Core State Machine & Models (`superstate-core`)

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/models.rs:1-80`
- Modify: `crates/superstate-core/src/state_machine.rs:70-130`
- Modify: `crates/superstate-core/src/lib.rs:1-30`
- Test: `crates/superstate-core/tests/test_state_milestones.rs`

**Interfaces:**
- Produces:
  - `superstate_core::models::MilestoneSpec`: `{ title: String, verify_cmd: String, files: Vec<PathBuf> }`
  - `superstate_core::models::ExecutionState.base_branch: Option<String>`
  - `superstate_core::state_machine::init_state_milestones(goal: Option<&str>, milestones: Vec<MilestoneSpec>, base_branch: Option<&str>, no_worktree: bool) -> Result<ExecutionState, StateError>`

**Step-by-Step Instructions:**
1. In `crates/superstate-core/tests/test_state_milestones.rs`, write failing tests:
   - `test_init_state_milestones_empty_fails`: Passing empty `vec![]` returns `Err(StateError::InvalidTransition(...))`.
   - `test_init_state_milestones_dag_construction`: Passing 3 milestones constructs `task-1` as `active_task`, and `task-2` and `task-3` in `pending_tasks`.
   - Asserts `task-2.depends_on == vec!["task-1"]` and `task-3.depends_on == vec!["task-2"]`.
   - Asserts `state.base_branch == Some("feat/stripe-webhooks".to_string())`.
   - Asserts `state.plan_path.is_none()`.
2. Run `cargo test -p superstate-core --test test_state_milestones` and confirm compile failure.
3. In `crates/superstate-core/src/models.rs`:
   - Define `MilestoneSpec` with `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]`.
   - Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub base_branch: Option<String>` to `ExecutionState`.
4. In `crates/superstate-core/src/state_machine.rs`:
   - Implement `init_state_milestones`:
     - Validate `!milestones.is_empty()`.
     - Validate each milestone has non-empty `title` and `verify_cmd`.
     - Build `task-1` from `milestones[0]` (id: `"task-1"`, title: milestone.title, depends_on: `vec![]`).
     - Build `task-2..N` from `milestones[1..]` (id: `format!("task-{}", i + 1)`, title: milestone.title, depends_on: `vec![format!("task-{}", i)]`).
     - Return `ExecutionState` with `active_task: Some(task_1)`, `pending_tasks: tasks_2_to_n`, `step: 1`, `status: ExecutionStatus::InProgress`, `base_branch: base_branch.map(ToString::to_string)`.
5. Re-export `MilestoneSpec` and `init_state_milestones` in `crates/superstate-core/src/lib.rs`.
6. Run `cargo test -p superstate-core` and verify all tests pass.
7. Commit: `git add crates/superstate-core && git commit -m "feat(core): add MilestoneSpec and init_state_milestones DAG initialization"`

---

### Task 2: Active Feature Branch Discovery & Worktree Targeting (`superstate-worktree`)

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-worktree/src/manager.rs:25-180`
- Modify: `crates/superstate-worktree/src/lib.rs:1-30`
- Test: `crates/superstate-worktree/tests/test_feature_branch_targeting.rs`

**Interfaces:**
- Consumes: `WorktreeManager`, `IsolatedWorktree`
- Produces:
  - `WorktreeManager::current_branch(&self) -> Result<String, WorktreeError>`
  - `ensure_task_worktree(&self, task_slug: &str, base_branch: Option<&str>)` defaulting to `current_branch()` instead of `"main"`.

**Step-by-Step Instructions:**
1. In `crates/superstate-worktree/tests/test_feature_branch_targeting.rs`, write failing tests:
   - `test_current_branch_detection`:
     - Creates test git repo, checks out branch `feat/test-feature`.
     - Asserts `manager.current_branch().unwrap() == "feat/test-feature"`.
   - `test_milestone_merges_to_feature_branch`:
     - Creates test repo on `feat/payment-integration`.
     - Provisions worktree for `task-1` with `base_branch: Some("feat/payment-integration")` (or None).
     - Commits a change in `.wt/task-1`.
     - Calls `wt.merge_and_cleanup("feat(task-1): add stripe client")`.
     - Checks out `feat/payment-integration` in primary repo and asserts the commit is present on `feat/payment-integration`, NOT `main`.
2. Run `cargo test -p superstate-worktree --test test_feature_branch_targeting` and verify failure.
3. In `crates/superstate-worktree/src/manager.rs`:
   - Implement `pub fn current_branch(&self) -> Result<String, WorktreeError>`:
     - Run `git -C <repo_root> rev-parse --abbrev-ref HEAD`.
     - If success and trimmed output is not `"HEAD"` (detached), return output.
     - If error or detached, fallback to `self.default_branch()`.
   - In `ensure_task_worktree(&self, task_slug: &str, base_branch: Option<&str>)`:
     - When `base_branch` is `None`, resolve base branch using `self.current_branch().unwrap_or_else(|_| self.default_branch())`.
4. Run `cargo test -p superstate-worktree` and confirm all tests pass.
5. Commit: `git add crates/superstate-worktree && git commit -m "feat(worktree): add current_branch detection and target active feature branch"`

---

### Task 3: CLI `--milestones` & Feature Branch Propagation (`superstate-cli`)

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: `crates/superstate-cli/src/state_cli.rs:30-80`
- Modify: `crates/superstate-cli/src/commands/state.rs:70-460`
- Test: `crates/superstate-cli/tests/test_cli_init_milestones.rs`

**Interfaces:**
- Consumes: `init_state_milestones`, `MilestoneSpec`, `WorktreeManager::current_branch`
- Produces:
  - `superstate state init --milestones <json_or_stdin>`
  - `superstate state next` worktree auto-merge into `state.base_branch` and sequential worktree provisioning.
  - `superstate state show` rendering `Base Branch`.

**Step-by-Step Instructions:**
1. In `crates/superstate-cli/tests/test_cli_init_milestones.rs`, write integration tests:
   - `test_cli_init_milestones_from_json_string`:
     - Runs CLI `state init --milestones '[{"title":"Step 1","verify_cmd":"true"},{"title":"Step 2","verify_cmd":"true"}]'`.
     - Inspects `.agent-state/state.json`: asserts `task-1` is active, `task-2` is pending, `base_branch` matches current repo branch.
   - `test_cli_init_milestones_from_stdin`:
     - Pipes JSON string to `state init --milestones -`.
     - Asserts successful initialization.
   - `test_cli_milestones_mutually_exclusive_with_plan_and_goal`:
     - Passing `--milestones` with `--plan` or `--goal` fails with exit code != 0.
2. Run `cargo test -p superstate --test test_cli_init_milestones` and verify test failure.
3. In `crates/superstate-cli/src/state_cli.rs`:
   - Add `#[arg(long, help = "JSON array of milestones or - for stdin")] pub milestones: Option<String>` to `InitArgs`.
4. In `crates/superstate-cli/src/commands/state.rs`:
   - In `cmd_init`:
     - Add mutually exclusive validation: exactly one of `--plan`, (`--goal` + `--verify-cmd`), or `--milestones` must be provided.
     - When `--milestones` is present:
       - If `milestones == "-"`, read string from `std::io::stdin()`.
       - Parse into `Vec<MilestoneSpec>`.
       - Detect `base_branch`: if git repo, `WorktreeManager::new(&root)?.current_branch().ok()`.
       - Call `init_state_milestones(args.goal.as_deref(), parsed_milestones, base_branch.as_deref(), args.no_worktree)`.
       - If `!args.no_worktree`, provision worktree for `task-1`.
       - Atomically save state to `state_path`.
   - In `cmd_next`:
     - When creating `IsolatedWorktree` for merge and cleanup, use `state.base_branch` if available (falling back to manager's `current_branch` or `default_branch`).
     - After `advance_step`: if `!state.no_worktree && root.join(".git").exists()`, and `state.active_task.is_some()`, provision the worktree for the newly active task and update `state.active_worktree` and `state.active_branch`.
   - In `cmd_show`:
     - If `state.base_branch` is present, display `- **Base Branch:** \`{}\``.
5. Run `cargo test -p superstate` and verify all tests pass.
6. Commit: `git add crates/superstate-cli && git commit -m "feat(cli): add --milestones flag and automatic sequential worktree progression"`

---

### Task 4: MCP Tool Dispatch & Multi-Milestone Progression (`superstate-mcp`)

- **Depends on:** Task 1, Task 2, Task 3

**Files:**
- Modify: `crates/superstate-mcp/src/tools.rs:80-124`
- Modify: `crates/superstate-mcp/src/server.rs:190-470`
- Test: `crates/superstate-mcp/tests/test_mcp_multi_milestones.rs`

**Interfaces:**
- Produces:
  - `superstate_handoff` supporting optional `milestones: [{title, verify_cmd, files}]`.
  - `superstate_preflight` supporting optional `verify_cmds: Vec<String>`.
  - Automatic worktree provisioning for next task in `state_next`.

**Step-by-Step Instructions:**
1. In `crates/superstate-mcp/tests/test_mcp_multi_milestones.rs`, write tests:
   - `test_mcp_superstate_handoff_with_milestones_array`:
     - Creates test repo with feature branch `feat/multi-task-flow`.
     - Calls `tools/call` for `superstate_handoff` with `milestones: [{"title": "T1", "verify_cmd": "echo 1"}, {"title": "T2", "verify_cmd": "echo 2"}]`.
     - Asserts response contains `task-1` turnkey prompt, and `.agent-state/state.json` has `base_branch == "feat/multi-task-flow"`, `active_task == "task-1"`, `pending_tasks == ["task-2"]`.
     - Asserts `.wt/task-1` exists.
   - `test_mcp_state_next_advances_and_provisions_task_2`:
     - Calls `state_next` on above state.
     - Asserts `.wt/task-1` is merged to `feat/multi-task-flow` and cleaned up.
     - Asserts `.wt/task-2` is provisioned.
     - Asserts `active_task` is `task-2`.
2. Run `cargo test -p superstate-mcp --test test_mcp_multi_milestones` and verify failure.
3. In `crates/superstate-mcp/src/tools.rs`:
   - Update `superstate_handoff` schema: add `milestones` array property.
   - Update `superstate_preflight` schema: add `verify_cmds` array property.
4. In `crates/superstate-mcp/src/server.rs`:
   - In `superstate_preflight`: if `verify_cmds` present, iterate and execute preflight on each command, returning aggregate results.
   - In `superstate_handoff`:
     - If `milestones` array is present in JSON args, deserialize into `Vec<MilestoneSpec>`.
     - Discover `base_branch` via `WorktreeManager::new(&repo_root)?.current_branch()`.
     - Call `init_state_milestones(goal, milestones, base_branch.as_deref(), no_worktree)`.
     - If `!no_worktree`, provision `.wt/task-1` via `WorktreeManager`.
     - Render and return turnkey prompt for `task-1`.
   - In `state_next`:
     - Resolve base branch from `state.base_branch` (or `manager.current_branch()`).
     - Merge `wt` into base branch and cleanup.
     - Call `advance_step(&mut state)`.
     - If `!state.no_worktree && state.active_task.is_some()`:
       - Provision `.wt/<next_task_id>` for the newly active task.
       - Update `state.active_worktree` and `state.active_branch`.
       - Render turnkey prompt for the next task and include in the `state_next` response content.
5. Run `cargo test -p superstate-mcp` and verify all tests pass.
6. Commit: `git add crates/superstate-mcp && git commit -m "feat(mcp): support multi-milestone handoff and sequential worktree transitions"`

---

### Task 5: End-to-End Verification & Documentation

- **Depends on:** Tasks 1–4

**Files:**
- Modify: `crates/superstate-cli/README.md` (or top-level docs)
- Run all workspace tests

**Step-by-Step Instructions:**
1. Run `cargo test --all` across the entire workspace (`superstate-core`, `superstate-worktree`, `superstate-cli`, `superstate-mcp`).
2. Run `cargo clippy --all -- -D warnings`.
3. Execute live CLI test in a temporary repository:
   - Initialize with 2 milestones.
   - Advance milestone 1 with `superstate state next`.
   - Verify milestone 1 commit appears on the feature branch.
   - Advance milestone 2 with `superstate state next`.
   - Verify both commits are present and state is `Completed`.
4. Commit: `git commit -am "chore: update documentation and verify multi-milestone direct handoff"`
