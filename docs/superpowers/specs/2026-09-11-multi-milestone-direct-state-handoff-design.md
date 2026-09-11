# Multi-Milestone Direct State Handoff & Plan-Free Engine Design

## Context & Motivation
Following the successful implementation of single-goal direct state handoff (which proved a 31% speedup on `openssl-selfsigned-cert` and eliminated 100% of the Phase 1 planning overhead), we need to extend this capability to **involved, multi-milestone tasks**.

In modern developer workflows using Claude Code or `agy`, requirements discovery, architectural exploration, and task decomposition occur interactively during the conversation. For an involved task, the human and agent agree upon a sequence of milestones with explicit verification invariants (for example: Phase 1: database migration, Phase 2: API endpoints, Phase 3: client wiring).

Authoring a 400-line markdown plan file on disk (`docs/superpowers/plans/feature-x.md`) to represent this agreement is an anti-pattern:
1. **Redundant Serialization**: Serializing structured decisions from chat into markdown only to parse them back with regexes into `.agent-state/state.json` introduces parsing errors and maintenance friction.
2. **Worktree Clutter**: Plan files dirty the working tree and trigger false positive failures under `--verify-git-clean`.
3. **Control Plane Confusion**: Developers do not want a "harness over a harness" or markdown bookkeeping; they want Claude Code / `agy` to hand off an agreed contract directly to Superstate's state machine via MCP.

Crucially, **milestone merges must target the developer's current feature branch** (e.g. `feat/stripe-webhooks`), never `main`, preserving clean, observable atomic commits that can be reviewed with `roborev` and pushed as a Pull Request.

---

## Goals
- Allow `superstate_handoff` in `superstate-mcp` to accept an ordered list of `milestones`, directly constructing a multi-task execution DAG in memory.
- Update `superstate state init` in `superstate-cli` to accept `--milestones <json>` (and stdin `-`) for multi-milestone direct initialization.
- Capture the developer's active Git branch at initialization time as `base_branch` in `ExecutionState`, ensuring all milestone worktree merges commit to the active feature branch rather than `main`.
- Automate multi-milestone worktree progression: as each milestone passes verification, its worktree merges into `base_branch`, cleans up, and the next milestone's worktree is provisioned from the newly updated `base_branch`.
- Provide dynamic rendering via `superstate state show`, allowing developers to inspect progress, pending tasks, and active worktrees without any markdown files existing on disk.
- Maintain 100% backward compatibility with single-milestone handoffs (`goal` + `verify_cmd`) and legacy markdown plan files (`--plan`).

## Non-Goals
- We are not eliminating multi-step decomposition; we are eliminating the redundant markdown plan file on disk.
- We are not altering external agent chat interfaces; Claude Code and `agy` remain the user-facing interface, using Superstate via MCP.

---

## Architectural Specification

### 1. Data Model & Core State Machine (`superstate-core`)

#### Typed Milestone Specification
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MilestoneSpec {
    pub title: String,
    pub verify_cmd: String,
    #[serde(default)]
    pub files: Vec<PathBuf>,
}
```

#### State Machine Construction (`init_state_milestones`)
```rust
pub fn init_state_milestones(
    goal: Option<&str>,
    milestones: Vec<MilestoneSpec>,
    base_branch: Option<&str>,
    no_worktree: bool,
) -> Result<ExecutionState, StateError>
```

**Invariants**:
- `milestones` must contain at least 1 milestone.
- Every milestone must provide a non-empty `title` and `verify_cmd`.
- `task-1` is derived from `milestones[0]` with `depends_on = []` and becomes `active_task`.
- `task-2..N` are derived from `milestones[1..]` with sequential dependency on `task-(i-1)` (`depends_on = [format!("task-{}", i)]`) and placed in `pending_tasks`.
- `ExecutionState.base_branch` is set to the caller-supplied branch or discovered active branch (falling back to `"main"` only if discovery fails).
- `ExecutionState.plan_path` is `None`.
- `ExecutionState.status` is `ExecutionStatus::InProgress`, `step = 1`.

---

### 2. Feature Branch Targeting & Worktree Lifecycle (`superstate-worktree`)

#### Active Branch Discovery
In `superstate-worktree::manager::WorktreeManager`:
```rust
pub fn current_branch(&self) -> Result<String, WorktreeError>
```
Executes `git rev-parse --abbrev-ref HEAD` in `repo_root`. If in detached HEAD or error, falls back to `default_branch()`.

#### Sequential Milestone Merging
When `state_next` verifies a task:
1. Target verification directory: `active_worktree` (e.g. `.wt/task-1`).
2. Gates run (`verify_cmd`, `verify_git_clean`).
3. On passing, `wt.merge_and_cleanup(commit_msg)` is invoked:
   - Commit message: `feat(task-1): <title>`.
   - Merges `wt/task-1` into `base_branch` (the user's feature branch).
   - Removes `.wt/task-1` directory and deletes branch `wt/task-1`.
4. `advance_step(&mut state)` is called:
   - `task-1` moved to `completed_tasks`.
   - `task-2` promoted to `active_task`.
5. If `!state.no_worktree`:
   - Provisions `.wt/task-2` branching off the newly updated `base_branch`.
   - Turnkey prompt for `task-2` is generated and returned.

---

### 3. MCP Tool Enhancements (`superstate-mcp`)

#### `superstate_handoff`
```json
{
  "name": "superstate_handoff",
  "description": "Directly initialize state and provision turnkey prompt from a goal or milestone list without plan.md",
  "input_schema": {
    "type": "object",
    "properties": {
      "goal": {
        "type": "string",
        "description": "Overall objective or PR title"
      },
      "verify_cmd": {
        "type": "string",
        "description": "Verification command for single-milestone task"
      },
      "files": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Target files for single milestone"
      },
      "milestones": {
        "type": "array",
        "description": "Ordered list of milestones for multi-step tasks",
        "items": {
          "type": "object",
          "properties": {
            "title": { "type": "string" },
            "verify_cmd": { "type": "string" },
            "files": { "type": "array", "items": { "type": "string" } }
          },
          "required": ["title", "verify_cmd"]
        }
      },
      "no_worktree": {
        "type": "boolean",
        "description": "If true, execute in-place without creating a worktree"
      }
    }
  }
}
```

#### `superstate_preflight`
Extended to optionally accept `verify_cmds: Vec<String>` to validate all milestone verification commands before handoff.

---

### 4. CLI Parity (`superstate-cli`)

#### Command Interface
```bash
# Direct multi-milestone initialization via JSON string
superstate state init --milestones '[{"title":"Add DB schema","verify_cmd":"cargo test -p db"},{"title":"Add API handler","verify_cmd":"cargo test -p api"}]'

# Direct multi-milestone initialization via stdin
cat milestones.json | superstate state init --milestones -
```

#### On-Demand View (`superstate state show`)
```markdown
# Superstate Execution Status
- **Status:** InProgress (Step 1/3)
- **Base Branch:** `feat/stripe-webhooks`
- **Active Task:** task-1: Add Redis idempotency cache layer
- **Verification Command:** `pytest tests/test_cache.py`
- **Active Worktree:** `.wt/task-1` (branch `wt/task-1`)
- **Pending Tasks (2):**
  - [ ] task-2: Hook up Stripe webhook receiver (depends on: task-1)
  - [ ] task-3: Add dead-letter queue replay worker (depends on: task-2)
```

---

## Verification & Test Plan

1. **Unit & State Machine Tests (`superstate-core`)**:
   - `test_init_state_milestones_empty_fails`: asserts error if empty milestones.
   - `test_init_state_milestones_dag_construction`: verifies `active_task` is `task-1`, `pending_tasks` are `task-2..N` with correct sequential dependencies.
   - `test_init_state_milestones_base_branch`: verifies custom or discovered base branch is recorded.

2. **Worktree Lifecycle Tests (`superstate-worktree`)**:
   - `test_current_branch_detection`: verifies current branch detection.
   - `test_sequential_milestone_merges_to_feature_branch`: verifies task 1 merges into feature branch, task 2 branches from updated feature branch, and final feature branch contains both atomic commits.

3. **CLI Integration Tests (`superstate-cli`)**:
   - `test_cli_init_milestones_json`: runs `superstate state init --milestones '...'`, checks `.agent-state/state.json`.
   - `test_cli_init_milestones_stdin`: pipes JSON into `superstate state init --milestones -`.

4. **MCP Protocol Tests (`superstate-mcp`)**:
   - `test_mcp_superstate_handoff_multi_milestones`: invokes `tools/call` for `superstate_handoff` with `milestones` array, asserts `task-1` active, `task-2` pending, `.wt/task-1` created.
   - `test_mcp_state_next_multi_milestone_progression`: verifies advancing task 1 merges to base branch and transitions to task 2.
