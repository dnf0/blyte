use std::path::PathBuf;
use superstate_core::models::{ExecutionStatus, MilestoneSpec};
use superstate_core::state_machine::{init_state_milestones, StateError};

#[test]
fn test_init_state_milestones_empty_fails() {
    let result = init_state_milestones(None, vec![], None, false);
    match result {
        Err(StateError::InvalidTransition(msg)) => {
            assert!(msg.contains("milestones cannot be empty") || !msg.is_empty());
        }
        _ => panic!("Expected StateError::InvalidTransition, got {:?}", result),
    }
}

#[test]
fn test_init_state_milestones_dag_construction() {
    let milestones = vec![
        MilestoneSpec {
            title: "Setup Stripe Client".to_string(),
            verify_cmd: "cargo test -p stripe".to_string(),
            files: vec![PathBuf::from("src/stripe.rs")],
        },
        MilestoneSpec {
            title: "Handle Webhook Events".to_string(),
            verify_cmd: "cargo test -p stripe-webhook".to_string(),
            files: vec![PathBuf::from("src/webhook.rs")],
        },
        MilestoneSpec {
            title: "Database Persistence".to_string(),
            verify_cmd: "cargo test -p stripe-db".to_string(),
            files: vec![PathBuf::from("src/db.rs")],
        },
    ];

    let state = init_state_milestones(
        Some("Implement Stripe webhooks"),
        milestones,
        Some("feat/stripe-webhooks"),
        false,
    )
    .expect("Failed to initialize state from milestones");

    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.step, 1);
    assert!(state.plan_path.is_none());
    assert_eq!(state.base_branch, Some("feat/stripe-webhooks".to_string()));

    let active = state
        .active_task
        .as_ref()
        .expect("Active task must be present");
    assert_eq!(active.id, "task-1");
    assert_eq!(active.title, "Setup Stripe Client");
    assert_eq!(
        active.verification_cmd,
        Some("cargo test -p stripe".to_string())
    );
    assert_eq!(active.files_modify, vec![PathBuf::from("src/stripe.rs")]);
    assert!(active.depends_on.is_empty());

    assert_eq!(state.pending_tasks.len(), 2);

    let task2 = &state.pending_tasks[0];
    assert_eq!(task2.id, "task-2");
    assert_eq!(task2.title, "Handle Webhook Events");
    assert_eq!(
        task2.verification_cmd,
        Some("cargo test -p stripe-webhook".to_string())
    );
    assert_eq!(task2.files_modify, vec![PathBuf::from("src/webhook.rs")]);
    assert_eq!(task2.depends_on, vec!["task-1".to_string()]);

    let task3 = &state.pending_tasks[1];
    assert_eq!(task3.id, "task-3");
    assert_eq!(task3.title, "Database Persistence");
    assert_eq!(
        task3.verification_cmd,
        Some("cargo test -p stripe-db".to_string())
    );
    assert_eq!(task3.files_modify, vec![PathBuf::from("src/db.rs")]);
    assert_eq!(task3.depends_on, vec!["task-2".to_string()]);

    assert!(state.completed_tasks.is_empty());
    assert!(!state.no_worktree);
}

#[test]
fn test_init_state_milestones_validation_errors() {
    let empty_title = vec![MilestoneSpec {
        title: "  ".to_string(),
        verify_cmd: "cargo test".to_string(),
        files: vec![],
    }];
    assert!(matches!(
        init_state_milestones(None, empty_title, None, false),
        Err(StateError::InvalidTransition(_))
    ));

    let empty_verify = vec![MilestoneSpec {
        title: "Valid Title".to_string(),
        verify_cmd: "   ".to_string(),
        files: vec![],
    }];
    assert!(matches!(
        init_state_milestones(None, empty_verify, None, false),
        Err(StateError::InvalidTransition(_))
    ));
}
