# Task 1 Brief: Model Enhancements & State Machine DAG Advancement

## Working Directory
`/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-dag-execution`
Branch: `feat/daemon-dag-execution`

## Goal
Extend `MilestoneSpec` in `crates/superstate-core/src/models.rs` to support optional explicit `id` and `depends_on`.
Upgrade `advance_step` and `init_state_milestones` in `crates/superstate-core/src/state_machine.rs` to resolve arbitrary DAGs, eliminate the blind `.unwrap_or(0)` fallback, and fail-closed with `ExecutionStatus::Blocked` when remaining pending tasks are deadlocked.

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies.
- Zero network I/O or direct API keys in Rust binary.
- Use `--no-gpg-sign` for git commits.
- Follow strict Test-Driven Development (TDD).

## Requirements

### 1. `crates/superstate-core/src/models.rs`
Update `MilestoneSpec`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MilestoneSpec {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub verify_cmd: String,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    pub files: Vec<PathBuf>,
}
```

### 2. `crates/superstate-core/src/state_machine.rs`
#### `init_state_milestones`
- Support custom `id`: `m.id.clone().unwrap_or_else(|| format!("task-{}", idx + 1))`.
- Support custom `depends_on`:
  - If `Some(deps)`, use `deps`.
  - If `None`, if `idx == 0`, `vec![]`; if `idx > 0`, `vec![prev_id]`.
- Build the task list. The first task whose `depends_on` are satisfied (or first task if independent) becomes `active_task`. The rest go into `pending_tasks`.

#### `advance_step`
- When `state.active_task` is taken, push to `state.completed_tasks`.
- If `!state.pending_tasks.is_empty()`:
  - Search `pending_tasks.iter().position(|t| t.depends_on.iter().all(|dep| is_dependency_satisfied(dep, &completed)))`.
  - If `Some(idx)`:
    - `state.active_worktree = None;`
    - `state.active_branch = None;`
    - `state.active_task = Some(state.pending_tasks.remove(idx));`
    - `state.step += 1;`
    - `state.status = ExecutionStatus::InProgress;`
  - If `None`:
    - `state.active_task = None;`
    - `state.status = ExecutionStatus::Blocked;`
    - Return `Err(StateError::InvalidTransition("Cannot advance: pending tasks exist but none have satisfied dependencies".to_string()))`.
- If `state.pending_tasks.is_empty()`:
  - `state.active_task = None;`
  - `state.status = ExecutionStatus::Completed;`

### 3. Integration Tests
Write tests in `crates/superstate-core/tests/test_state_machine_dag.rs`:
- `test_init_state_milestones_with_custom_dag_dependencies`
- `test_advance_step_resolves_diamond_dag_topologically`
- `test_advance_step_deadlock_halts_with_blocked_status`

## Verification
- `cargo test -p superstate-core --test test_state_machine_dag`
- `cargo test -p superstate-core`
- `cargo check --all`

## Completion
- Commit with: `git commit --no-gpg-sign -m "feat(core): implement DAG milestone resolution and deadlock detection in advance_step"`
- Write detailed report to `docs/superpowers/plans/task-1-report.md`.
