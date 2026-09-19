use std::path::PathBuf;
use superstate_core::models::{ExecutionState, ExecutionStatus, MilestoneSpec, TaskSpec};
use superstate_core::state_machine::{advance_step, init_state_milestones, StateError};

#[test]
fn test_init_state_milestones_with_custom_dag_dependencies() {
    let milestones = vec![
        MilestoneSpec {
            id: Some("prep".to_string()),
            title: "Preparation".to_string(),
            verify_cmd: "cargo test --test prep".to_string(),
            depends_on: None, // should default to vec![] for idx == 0
            files: vec![PathBuf::from("src/prep.rs")],
        },
        MilestoneSpec {
            id: Some("worker-a".to_string()),
            title: "Worker A".to_string(),
            verify_cmd: "cargo test --test a".to_string(),
            depends_on: Some(vec!["prep".to_string()]),
            files: vec![PathBuf::from("src/worker_a.rs")],
        },
        MilestoneSpec {
            id: Some("worker-b".to_string()),
            title: "Worker B".to_string(),
            verify_cmd: "cargo test --test b".to_string(),
            depends_on: Some(vec!["prep".to_string()]),
            files: vec![PathBuf::from("src/worker_b.rs")],
        },
        MilestoneSpec {
            id: None, // should default to task-4
            title: "Worker C (Sequential Fallback)".to_string(),
            verify_cmd: "cargo test --test c".to_string(),
            depends_on: None, // should default to prev_id ("worker-b")
            files: vec![PathBuf::from("src/worker_c.rs")],
        },
    ];

    let state = init_state_milestones(
        Some("Run custom DAG"),
        milestones,
        Some("feat/custom-dag"),
        false,
    )
    .expect("Failed to initialize state with custom DAG milestones");

    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.step, 1);
    assert_eq!(state.base_branch, Some("feat/custom-dag".to_string()));
    assert_eq!(state.goal, Some("Run custom DAG".to_string()));

    let active = state
        .active_task
        .as_ref()
        .expect("Active task must be set");
    assert_eq!(active.id, "prep");
    assert_eq!(active.title, "Preparation");
    assert!(active.depends_on.is_empty());

    assert_eq!(state.pending_tasks.len(), 3);

    let task_a = &state.pending_tasks[0];
    assert_eq!(task_a.id, "worker-a");
    assert_eq!(task_a.depends_on, vec!["prep".to_string()]);

    let task_b = &state.pending_tasks[1];
    assert_eq!(task_b.id, "worker-b");
    assert_eq!(task_b.depends_on, vec!["prep".to_string()]);

    let task_c = &state.pending_tasks[2];
    assert_eq!(task_c.id, "task-4");
    assert_eq!(task_c.depends_on, vec!["worker-b".to_string()]);
}

#[test]
fn test_advance_step_resolves_diamond_dag_topologically() {
    // Diamond DAG:
    //      task-a
    //      /    \
    //  task-b  task-c
    //      \    /
    //      task-d
    // To ensure topological resolution doesn't just happen by luck of index 0,
    // pending tasks are ordered: [task-d, task-b, task-c].
    let task_a = TaskSpec {
        id: "task-a".to_string(),
        title: "Task A".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        verification_cmd: Some("cargo test -p task-a".to_string()),
    };
    let task_b = TaskSpec {
        id: "task-b".to_string(),
        title: "Task B".to_string(),
        depends_on: vec!["task-a".to_string()],
        files_modify: vec![],
        verification_cmd: Some("cargo test -p task-b".to_string()),
    };
    let task_c = TaskSpec {
        id: "task-c".to_string(),
        title: "Task C".to_string(),
        depends_on: vec!["task-a".to_string()],
        files_modify: vec![],
        verification_cmd: Some("cargo test -p task-c".to_string()),
    };
    let task_d = TaskSpec {
        id: "task-d".to_string(),
        title: "Task D".to_string(),
        depends_on: vec!["task-b".to_string(), "task-c".to_string()],
        files_modify: vec![],
        verification_cmd: Some("cargo test -p task-d".to_string()),
    };

    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task_a),
        // Note: task_d is placed first in pending_tasks to strictly test dependency resolution
        pending_tasks: vec![task_d, task_b, task_c],
        ..Default::default()
    };

    // Step 1: Complete task-a. Among [task-d, task-b, task-c], task-b is the first satisfied.
    advance_step(&mut state).expect("Advance step after task-a failed");
    assert_eq!(state.step, 2);
    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.completed_tasks.len(), 1);
    assert_eq!(state.completed_tasks[0].id, "task-a");
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-b");
    // Remaining pending: [task-d, task-c]
    assert_eq!(state.pending_tasks.len(), 2);
    assert_eq!(state.pending_tasks[0].id, "task-d");
    assert_eq!(state.pending_tasks[1].id, "task-c");

    // Step 2: Complete task-b. Among [task-d, task-c], task-d still needs task-c, so task-c is selected.
    advance_step(&mut state).expect("Advance step after task-b failed");
    assert_eq!(state.step, 3);
    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.completed_tasks.len(), 2);
    assert_eq!(state.completed_tasks[1].id, "task-b");
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-c");
    // Remaining pending: [task-d]
    assert_eq!(state.pending_tasks.len(), 1);
    assert_eq!(state.pending_tasks[0].id, "task-d");

    // Step 3: Complete task-c. Now task-d has both dependencies (task-b and task-c) satisfied.
    advance_step(&mut state).expect("Advance step after task-c failed");
    assert_eq!(state.step, 4);
    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.completed_tasks.len(), 3);
    assert_eq!(state.completed_tasks[2].id, "task-c");
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-d");
    assert_eq!(state.pending_tasks.len(), 0);

    // Step 4: Complete task-d. All tasks are completed.
    advance_step(&mut state).expect("Advance step after task-d failed");
    assert_eq!(state.step, 5);
    assert_eq!(state.status, ExecutionStatus::Completed);
    assert!(state.active_task.is_none());
    assert_eq!(state.completed_tasks.len(), 4);
}

#[test]
fn test_advance_step_deadlock_halts_with_blocked_status() {
    let task_1 = TaskSpec {
        id: "task-1".to_string(),
        title: "Task 1".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        verification_cmd: None,
    };
    // Circular dependency: task-2 depends on task-3, task-3 depends on task-2
    let task_2 = TaskSpec {
        id: "task-2".to_string(),
        title: "Task 2".to_string(),
        depends_on: vec!["task-3".to_string()],
        files_modify: vec![],
        verification_cmd: None,
    };
    let task_3 = TaskSpec {
        id: "task-3".to_string(),
        title: "Task 3".to_string(),
        depends_on: vec!["task-2".to_string()],
        files_modify: vec![],
        verification_cmd: None,
    };

    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task_1),
        pending_tasks: vec![task_2, task_3],
        ..Default::default()
    };

    let result = advance_step(&mut state);
    match result {
        Err(StateError::InvalidTransition(msg)) => {
            assert!(
                msg.contains("Cannot advance: pending tasks exist but none have satisfied dependencies"),
                "Unexpected error message: {}",
                msg
            );
        }
        _ => panic!("Expected StateError::InvalidTransition, got: {:?}", result),
    }

    assert_eq!(state.status, ExecutionStatus::Blocked);
    assert!(state.active_task.is_none());
    assert_eq!(state.completed_tasks.len(), 1);
    assert_eq!(state.completed_tasks[0].id, "task-1");
    assert_eq!(state.pending_tasks.len(), 2);
}
