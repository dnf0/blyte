//! Plan reconciliation, diagnostics and patching.

use serde_json::json;
use superstate_core::maintenance::{diagnose, metrics, repair, sync_with_plan};
use superstate_core::models::{ExecutionState, ExecutionStatus};
use superstate_core::patching::{apply_patch, decompose_plan, validate_state};
use superstate_core::plan_parser::parse_plan_markdown;

const PLAN: &str =
    "### Task 1: Alpha\n- **Depends on:** none\n### Task 2: Beta\n- **Depends on:** Task 1\n";

fn state_from(plan: &str) -> ExecutionState {
    let mut tasks = parse_plan_markdown(plan).unwrap();
    let active = if tasks.is_empty() {
        None
    } else {
        Some(tasks.remove(0))
    };
    ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: active,
        pending_tasks: tasks,
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: None,
        notes: Vec::new(),
    }
}

#[test]
fn sync_preserves_completed_work_when_the_plan_changes() {
    // Completed work exists on disk regardless of what the plan now says, so it
    // must never be scheduled again.
    let mut state = state_from(PLAN);
    let done = state.active_task.take().unwrap();
    state.completed_tasks.push(done);
    state.active_task = Some(state.pending_tasks.remove(0));

    let grown = format!("{}### Task 3: Gamma\n- **Depends on:** Task 2\n", PLAN);
    let report = sync_with_plan(&mut state, &grown).unwrap();

    assert_eq!(report.added, vec!["Task 3: Gamma"]);
    assert_eq!(report.preserved_completed, 1);
    assert!(
        !state.pending_tasks.iter().any(|t| t.id == "Task 1"),
        "a completed task must not return to the pending set"
    );
}

#[test]
fn sync_drops_tasks_removed_from_the_plan() {
    let mut state = state_from(PLAN);
    let report = sync_with_plan(&mut state, "### Task 1: Alpha\n- **Depends on:** none\n").unwrap();
    assert_eq!(report.removed, vec!["Task 2: Beta"]);
    assert_eq!(state.pending_tasks.len(), 0);
}

#[test]
fn doctor_flags_a_task_that_is_both_done_and_outstanding() {
    let mut state = state_from(PLAN);
    state
        .completed_tasks
        .push(state.active_task.clone().unwrap());

    let report = diagnose(&state, None);
    assert!(!report.is_healthy);
    assert!(report.issues[0].contains("both completed and outstanding"));
}

#[test]
fn repair_promotes_a_pending_task_when_nothing_is_active() {
    let mut state = state_from(PLAN);
    state.active_task = None;
    let actions = repair(&mut state);
    assert!(state.active_task.is_some());
    assert!(actions.iter().any(|a| a.contains("Promoted")));
}

#[test]
fn metrics_report_progress() {
    let mut state = state_from(PLAN);
    let done = state.active_task.take().unwrap();
    state.completed_tasks.push(done);
    state.active_task = Some(state.pending_tasks.remove(0));

    let m = metrics(&state, 0);
    assert_eq!(m.tasks_total, 2);
    assert_eq!(m.tasks_completed, 1);
    assert_eq!(m.completion_pct, 50.0);
}

#[test]
fn a_patch_null_deletes_a_key_rather_than_setting_it_to_null() {
    let mut target = json!({"keep": 1, "drop": 2});
    apply_patch(&mut target, &json!({"drop": null, "add": 3}));
    assert_eq!(target, json!({"keep": 1, "add": 3}));
}

#[test]
fn a_patch_merges_nested_objects_rather_than_replacing_them() {
    let mut target = json!({"outer": {"a": 1, "b": 2}});
    apply_patch(&mut target, &json!({"outer": {"b": 9}}));
    assert_eq!(target, json!({"outer": {"a": 1, "b": 9}}));
}

#[test]
fn validation_reports_every_problem_not_just_the_first() {
    let errors = validate_state(&json!({"step": "one"}));
    assert!(
        errors.len() > 1,
        "expected several problems, got {:?}",
        errors
    );
    assert!(errors.iter().any(|e| e.contains("'status'")));
    assert!(errors
        .iter()
        .any(|e| e.contains("must be a non-negative integer")));
}

#[test]
fn decompose_splits_a_multi_file_task_into_sequential_children() {
    let plan = "### Task 1: Alpha\n- **Depends on:** none\n**Files:**\n- Modify: `a.rs`\n- Modify: `b.rs`\n";
    let out = decompose_plan(plan).unwrap();
    assert!(out.contains("### Task 1.1:"));
    assert!(out.contains("### Task 1.2:"));
    // The second child waits on the first: they touch the same task's surface.
    assert!(out.contains("**Depends on:** Task 1.1"));
}

#[test]
fn decompose_leaves_an_already_atomic_task_alone() {
    let plan = "### Task 1: Alpha\n- **Depends on:** none\n**Files:**\n- Modify: `a.rs`\n";
    let out = decompose_plan(plan).unwrap();
    assert!(out.contains("### Task 1:"));
    assert!(!out.contains("Task 1.1"));
}
