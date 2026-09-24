use blyte_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use blyte_core::state_machine::{advance_step, init_state_direct, load_state, save_state_atomic};
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_state_atomic_save_and_load() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Initial Task".to_string(),
            depends_on: vec![],
            files_modify: vec![PathBuf::from("src/lib.rs")],
            verification_cmd: Some("cargo test".to_string()),
        }),
        ..Default::default()
    };

    save_state_atomic(&state_file, &state).expect("Atomic save failed");
    assert!(state_file.exists());

    let loaded = load_state(&state_file).expect("Failed to load state");
    assert_eq!(loaded.step, 1);
    assert_eq!(loaded.active_task.unwrap().title, "Initial Task");
}

#[test]
fn test_advance_step_lifecycle() {
    let task1 = TaskSpec {
        id: "task-1".to_string(),
        title: "Task 1".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        verification_cmd: None,
    };
    let task2 = TaskSpec {
        id: "task-2".to_string(),
        title: "Task 2".to_string(),
        depends_on: vec!["task-1".to_string()],
        files_modify: vec![],
        verification_cmd: None,
    };

    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task1),
        pending_tasks: vec![task2],
        ..Default::default()
    };

    advance_step(&mut state).expect("Advance step failed");

    assert_eq!(state.step, 2);
    assert_eq!(state.completed_tasks.len(), 1);
    assert_eq!(state.completed_tasks[0].id, "task-1");
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-2");
    assert_eq!(state.pending_tasks.len(), 0);
    assert_eq!(state.status, ExecutionStatus::InProgress);

    // Advance again to complete the final task
    advance_step(&mut state).expect("Advance step failed");
    assert_eq!(state.step, 3);
    assert_eq!(state.completed_tasks.len(), 2);
    assert!(state.active_task.is_none());
    assert_eq!(state.status, ExecutionStatus::Completed);
}

#[test]
fn test_worktree_tracking_serialization_and_advance_reset() {
    let task1 = TaskSpec {
        id: "task-1".to_string(),
        title: "Task 1".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        verification_cmd: None,
    };
    let task2 = TaskSpec {
        id: "task-2".to_string(),
        title: "Task 2".to_string(),
        depends_on: vec!["task-1".to_string()],
        files_modify: vec![],
        verification_cmd: None,
    };

    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task1),
        pending_tasks: vec![task2],
        active_worktree: Some(PathBuf::from(".wt/task-1")),
        active_branch: Some("wt/task-1".to_string()),
        ..Default::default()
    };

    // Assert serialization and deserialization preserves active_worktree and active_branch
    let serialized = serde_json::to_string(&state).expect("Failed to serialize state");
    let deserialized: ExecutionState =
        serde_json::from_str(&serialized).expect("Failed to deserialize state");
    assert_eq!(
        deserialized.active_worktree,
        Some(PathBuf::from(".wt/task-1"))
    );
    assert_eq!(deserialized.active_branch, Some("wt/task-1".to_string()));

    // Assert backward compatibility: missing active_worktree/active_branch deserialize as None
    let json_without_worktree = r#"{
        "status": "in_progress",
        "step": 1
    }"#;
    let legacy_state: ExecutionState =
        serde_json::from_str(json_without_worktree).expect("Failed to deserialize legacy state");
    assert_eq!(legacy_state.active_worktree, None);
    assert_eq!(legacy_state.active_branch, None);

    // Assert advance_step resets active_worktree and active_branch to None
    advance_step(&mut state).expect("Advance step failed");
    assert_eq!(state.step, 2);
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-2");
    assert_eq!(state.active_worktree, None);
    assert_eq!(state.active_branch, None);
}

#[test]
fn test_init_state_direct() {
    let state = init_state_direct(
        "Build parser",
        "cargo test -p parser",
        vec![PathBuf::from("src/parser.rs")],
        true,
    );

    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.step, 1);
    assert!(state.plan_path.is_none());
    assert!(state.no_worktree);
    assert_eq!(state.goal, Some("Build parser".to_string()));
    assert!(state.pending_tasks.is_empty());
    assert!(state.completed_tasks.is_empty());

    let task = state.active_task.expect("active_task must be Some");
    assert_eq!(task.id, "task-1");
    assert_eq!(task.title, "Build parser");
    assert_eq!(
        task.verification_cmd,
        Some("cargo test -p parser".to_string())
    );
    assert_eq!(task.files_modify, vec![PathBuf::from("src/parser.rs")]);
}

#[test]
fn test_advance_step_selects_ready_task_in_dag_order() {
    let task1 = TaskSpec {
        id: "task-1".to_string(),
        title: "Setup foundation".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        verification_cmd: None,
    };
    // task-3 listed first in pending, but depends on task-2
    let task3 = TaskSpec {
        id: "task-3".to_string(),
        title: "Final integration".to_string(),
        depends_on: vec!["task-2".to_string()],
        files_modify: vec![],
        verification_cmd: None,
    };
    // task-2 listed second in pending, but depends on task-1 (which is active/completed first)
    let task2 = TaskSpec {
        id: "task-2".to_string(),
        title: "Core logic".to_string(),
        depends_on: vec!["task-1".to_string()],
        files_modify: vec![],
        verification_cmd: None,
    };

    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task1),
        pending_tasks: vec![task3, task2], // out of topological order
        ..Default::default()
    };

    // Advancing should pick task-2 because task-1 is completed and task-2's dependency is satisfied,
    // whereas task-3 is blocked waiting on task-2.
    advance_step(&mut state).expect("Advance step failed");
    assert_eq!(state.step, 2);
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-2");
    assert_eq!(state.pending_tasks.len(), 1);
    assert_eq!(state.pending_tasks[0].id, "task-3");

    // Advancing again completes task-2, unblocking task-3
    advance_step(&mut state).expect("Advance step failed");
    assert_eq!(state.step, 3);
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-3");
    assert_eq!(state.pending_tasks.len(), 0);

    // Final advance finishes the plan
    advance_step(&mut state).expect("Advance step failed");
    assert_eq!(state.status, ExecutionStatus::Completed);
    assert!(state.active_task.is_none());
}
