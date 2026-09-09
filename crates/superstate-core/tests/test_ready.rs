//! DAG readiness: which tasks may run now.
//!
//! Verified against the reference implementation (`agent-rules state ready --json`),
//! which returns the same task set at every step of this fixture.

use superstate_core::models::{ExecutionState, ExecutionStatus};
use superstate_core::plan_parser::parse_plan_markdown;
use superstate_core::state_machine::ready_tasks;

/// Build the state a fresh `state init` would produce, without touching disk.
fn state_from(plan: &str) -> ExecutionState {
    let mut tasks = parse_plan_markdown(plan).expect("plan parses");
    let active_task = if tasks.is_empty() {
        None
    } else {
        Some(tasks.remove(0))
    };
    ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task,
        pending_tasks: tasks,
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: None,
        notes: Vec::new(),
    }
}

const DAG_PLAN: &str = "\
### Task 1: Core abstractions
- **Depends on:** none
### Task 2: Backend adapter
- **Depends on:** Task 1
### Task 3: CLI interface
- **Depends on:** Task 1
### Task 4: Integration testing
- **Depends on:** Task 2, Task 3
";

fn titles(state: &ExecutionState) -> Vec<String> {
    ready_tasks(state)
        .iter()
        .map(|t| format!("{}: {}", t.id, t.title))
        .collect()
}

#[test]
fn only_dependency_free_tasks_are_ready_initially() {
    let state = state_from(DAG_PLAN);
    assert_eq!(titles(&state), vec!["Task 1: Core abstractions"]);
}

#[test]
fn completing_a_task_unblocks_everything_that_depended_on_it() {
    let mut state = state_from(DAG_PLAN);

    // Complete Task 1: both Task 2 and Task 3 depend on it alone.
    let task1 = state.active_task.take().expect("active task");
    state.completed_tasks.push(task1);
    state.active_task = Some(state.pending_tasks.remove(0));

    assert_eq!(
        titles(&state),
        vec!["Task 2: Backend adapter", "Task 3: CLI interface"],
        "a task with a satisfied dependency must become ready"
    );
}

#[test]
fn a_task_waiting_on_several_dependencies_stays_blocked_until_all_are_done() {
    let mut state = state_from(DAG_PLAN);

    // Complete Tasks 1 and 2. Task 4 needs 2 AND 3, so it must stay blocked.
    let task1 = state.active_task.take().expect("active task");
    state.completed_tasks.push(task1);
    let task2 = state.pending_tasks.remove(0);
    state.completed_tasks.push(task2);
    state.active_task = Some(state.pending_tasks.remove(0));

    let ready = titles(&state);
    assert!(ready.contains(&"Task 3: CLI interface".to_string()));
    assert!(
        !ready.iter().any(|t| t.starts_with("Task 4")),
        "Task 4 depends on Task 2 and Task 3; Task 3 is not done, so it is not ready: {:?}",
        ready
    );
}

#[test]
fn the_none_placeholder_is_not_treated_as_a_dependency() {
    let state = state_from("### Task 1: Solo\n- **Depends on:** none\n");
    assert_eq!(titles(&state).len(), 1, "`none` must mean no dependency");
}
