# Autonomous Daemon Dependency DAG Execution Design Specification

- **Document Name:** Autonomous Daemon Dependency DAG Execution
- **Date:** 2026-09-19
- **Status:** Approved
- **Author:** Antigravity / Google DeepMind
- **Target Repository:** `climate-x-org/superstate` (`crates/superstate-core`, `crates/superstate-cli`)

---

## 1. Executive Summary & Problem Statement

### 1.1 Context
In `docs/SYSTEM-REVIEW.md`, **Loss 1** identified that while `TaskSpec.depends_on` and `ready_tasks()` existed to model dependency graphs, the execution engine never actually used them to decide what runs next. Instead:
1. In `crates/superstate-core/src/state_machine.rs`, `advance_step()` evaluated dependencies but fell back to `.unwrap_or(0)` whenever no task had satisfied dependencies, blindly popping `pending_tasks[0]`.
2. In `crates/superstate-core/src/daemon.rs`, the background runner executed single atomic milestones. While the daemon has matured with blocking review, deterministic git rollback, and tool output shunting, it lacked end-to-end multi-milestone DAG orchestration.

### 1.2 Core Objective
Upgrade `superstate-core` and `superstate-cli` so `DaemonRunner` autonomously executes multi-milestone dependency DAGs in topological order with:
- Deterministic ready-queue task selection.
- Fail-closed deadlock detection (transitioning to `ExecutionStatus::Blocked` instead of popping unready tasks).
- Scoped milestone commit baselines and rollback isolation (rolling back Milestone 2 preserves Milestone 1's checkpointed work).
- Support for arbitrary DAG definitions (linear, branching, diamond) in `MilestoneSpec` and CLI `--milestones`.

---

## 2. Goals & Non-Goals

### 2.1 Goals
- **Topological Task Advancement:** `advance_step` strictly transitions to the first ready task whose `depends_on` entries are fully satisfied in `completed_tasks`.
- **Deadlock / Blocked Graph Detection:** If `pending_tasks` is non-empty but zero tasks are ready, `advance_step` transitions state to `Blocked` and returns an explicit error.
- **Milestone Rollback Isolation:** When Milestone $N$ fails verification under `--auto-rollback`, `git reset --hard` resets only to Milestone $N$'s start commit, preserving all commits from Milestones $1 \dots N-1$.
- **Arbitrary DAG Support in `MilestoneSpec`:** Support explicit `id` and `depends_on` in `MilestoneSpec`, while defaulting to sequential chaining when `depends_on` is omitted for 100% backward compatibility.
- **Zero New Dependencies:** Pure standard library and existing workspace crates (`serde`, `serde_json`, `chrono`).

### 2.2 Non-Goals
- **In-flight AST Test Impact Slicing:** Deferred to Initiative 2 (`impact.rs`).
- **Native Git Worktree Lifecycle:** Deferred to Initiative 3 (`WorktreeManager`).
- **Concurrent Worker Spawning:** The daemon remains single-threaded, executing one ready milestone at a time in topological order.

---

## 3. Architecture & Data Flow

### 3.1 Milestone Lifecycle State Machine

```
              ┌───────────────────────────┐
              │      State Initialized     │
              │  (active_task = ready[0]) │
              └─────────────┬─────────────┘
                            │
                            ▼
              ┌───────────────────────────┐
              │   Milestone Start Commit  │ ◄───────┐
              │  Record HEAD for active   │         │
              └─────────────┬─────────────┘         │
                            │                       │
                            ▼                       │
              ┌───────────────────────────┐         │ (auto-rollback
              │    Driver Worker Turn     │         │  resets to start commit)
              │  (Shunted tool execution) │         │
              └─────────────┬─────────────┘         │
                            │                       │
                            ▼                       │
              ┌───────────────────────────┐         │
              │  Run Verification Gate    │ ──Fail──┘ (retry budget > 0)
              └─────────────┬─────────────┘
                            │ Pass
                            ▼
              ┌───────────────────────────┐
              │   Git Checkpoint Commit   │
              │ chore(superstate): checkpt│
              └─────────────┬─────────────┘
                            │
                            ▼
              ┌───────────────────────────┐
              │    advance_step() (DAG)   │
              └──────┬──────────────┬─────┘
                     │              │
      Ready task     │              │ All done
      available      │              ▼
                     │       ┌──────────────┐
                     │       │  Completed   │
                     │       └──────────────┘
                     │
                     ▼ Deadlock / No ready tasks
              ┌──────────────┐
              │   Blocked    │
              └──────────────┘
```

---

## 4. Detailed Component Design

### 4.1 Data Model: `MilestoneSpec` Enhancements
In `crates/superstate-core/src/models.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MilestoneSpec {
    /// Optional explicit milestone identifier (e.g. "task-1", "setup", "routes").
    /// Defaults to "task-{idx+1}" if omitted.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub verify_cmd: String,
    /// Explicit dependency milestone IDs.
    /// If None, defaults to depending sequentially on the previous milestone.
    /// If Some(vec![]), indicates an independent root task.
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    pub files: Vec<PathBuf>,
}
```

### 4.2 State Machine: `init_state_milestones`
In `crates/superstate-core/src/state_machine.rs`:
- Constructs `TaskSpec` for each milestone.
- Resolves `id`: uses `milestone.id` or `format!("task-{}", idx + 1)`.
- Resolves `depends_on`:
  - If `Some(deps)`: uses `deps`.
  - If `None`: if `idx == 0`, `vec![]`; if `idx > 0`, `vec![prev_id]`.
- Identifies the initial active task:
  - Evaluates `ready_tasks(&state)`.
  - The first ready task becomes `state.active_task`; remaining tasks become `state.pending_tasks`.

### 4.3 State Machine: `advance_step`
In `crates/superstate-core/src/state_machine.rs`:
```rust
pub fn advance_step(state: &mut ExecutionState) -> Result<(), StateError> {
    if let Some(completed) = state.active_task.take() {
        state.completed_tasks.push(completed);
    }

    if !state.pending_tasks.is_empty() {
        let completed: Vec<&str> = state
            .completed_tasks
            .iter()
            .flat_map(|t| [t.id.as_str(), t.title.as_str()])
            .collect();

        // Advance to the first task whose dependencies are satisfied in DAG order
        let next_idx = state
            .pending_tasks
            .iter()
            .position(|task| {
                task.depends_on
                    .iter()
                    .all(|dep| is_dependency_satisfied(dep, &completed))
            });

        match next_idx {
            Some(idx) => {
                state.active_worktree = None;
                state.active_branch = None;
                state.active_task = Some(state.pending_tasks.remove(idx));
                state.step += 1;
                state.status = ExecutionStatus::InProgress;
            }
            None => {
                state.active_task = None;
                state.status = ExecutionStatus::Blocked;
                return Err(StateError::InvalidTransition(
                    "Cannot advance: pending tasks exist but none have satisfied dependencies".to_string(),
                ));
            }
        }
    } else {
        state.active_task = None;
        state.status = ExecutionStatus::Completed;
    }
    Ok(())
}
```

### 4.4 Daemon Runner: Multi-Milestone Execution Lifecycle
In `crates/superstate-core/src/daemon.rs`:
1. **Milestone Baseline Tracking:**
   `self.milestone_start_commits.entry(active_task.id).or_insert(rev_parse_head)`.
2. **Checkpoint Commits:**
   On gate pass in git repo with `state.no_worktree`:
   Commit dirty tree with `chore(superstate): checkpoint completed milestone {id}: {title}`.
3. **Rollback Scoping:**
   `perform_rollback` uses `self.milestone_start_commits.get(&active_task.id)`. Reverts solely to the commit at the start of that milestone.
4. **DAG Advance & Deadlock Handling:**
   Call `advance_step(&mut state)`.
   - On `Ok(())`: if `state.status == Completed`, finalize and emit `DaemonStepOutcome::Advanced { next_task_id: None }`. If `InProgress`, emit `DaemonStepOutcome::Advanced { next_task_id: Some(...) }`.
   - On `Err(StateError::InvalidTransition(msg))`: record `[dag-blocked]` note in ledger, save state, and emit `DaemonStepOutcome::Blocked { task_id, reason: msg }`.

---

## 5. Verification & Testing Strategy

### 5.1 Unit Tests (`superstate-core`)
- `test_advance_step_topological_resolution`: Diamond DAG (T1 -> T2A, T2B -> T3). Verify T2A and T2B execute before T3 regardless of pending order.
- `test_advance_step_deadlock_detection`: Pending tasks with circular or unsatisfied dependencies transition to `ExecutionStatus::Blocked`.
- `test_init_state_milestones_explicit_dag`: Initializing with explicit `depends_on` and custom IDs.

### 5.2 Integration Tests (`superstate-core/tests/test_daemon_dag_execution.rs`)
- **Linear Multi-Milestone Chain:** 3 milestones with real shell verification scripts. Verify all 3 pass, 3 git checkpoint commits created, state reaches `Completed`.
- **Milestone Rollback Isolation:** Milestone 1 passes. Milestone 2 fails verification twice and rolls back. Verify Milestone 1 files and commit remain completely intact. Milestone 2 passes on attempt 3.
- **Deadlock Halt:** Milestone 2 depends on non-existent `task-99`. Verify daemon halts with `DaemonStepOutcome::Blocked` without crashing.

### 5.3 CLI Integration Tests (`superstate-cli/tests/test_cli_dag_daemon.rs`)
- `superstate state init --milestones '[...]'` with diamond dependencies.
- `superstate state daemon` runs through the full DAG to completion.
