use std::path::PathBuf;
use superstate_core::models::{ExecutionState, ExecutionStatus, MilestoneSpec};
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
            ..Default::default()
        },
        MilestoneSpec {
            title: "Handle Webhook Events".to_string(),
            verify_cmd: "cargo test -p stripe-webhook".to_string(),
            files: vec![PathBuf::from("src/webhook.rs")],
            ..Default::default()
        },
        MilestoneSpec {
            title: "Database Persistence".to_string(),
            verify_cmd: "cargo test -p stripe-db".to_string(),
            files: vec![PathBuf::from("src/db.rs")],
            ..Default::default()
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
    assert_eq!(state.goal, Some("Implement Stripe webhooks".to_string()));

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
        ..Default::default()
    }];
    assert!(matches!(
        init_state_milestones(None, empty_title, None, false),
        Err(StateError::InvalidTransition(_))
    ));

    let empty_verify = vec![MilestoneSpec {
        title: "Valid Title".to_string(),
        verify_cmd: "   ".to_string(),
        files: vec![],
        ..Default::default()
    }];
    assert!(matches!(
        init_state_milestones(None, empty_verify, None, false),
        Err(StateError::InvalidTransition(_))
    ));
}

#[test]
fn test_init_state_milestones_single_milestone() {
    // When exactly one milestone is provided, it should immediately become the active task
    // without any pending downstream tasks in the execution queue.
    let milestones = vec![MilestoneSpec {
        title: "Single task".to_string(),
        verify_cmd: "cargo test".to_string(),
        files: vec![PathBuf::from("src/single.rs")],
        ..Default::default()
    }];

    // Initialize state with single milestone and verify immediate transition to task-1
    let state = init_state_milestones(
        Some("Single milestone execution"),
        milestones,
        Some("feat/single"),
        false,
    )
    .expect("State initialization with a single milestone must succeed");

    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.step, 1);

    // Active task must be task-1 with matching attributes
    let active = state
        .active_task
        .as_ref()
        .expect("Active task must be set to task-1");
    assert_eq!(active.id, "task-1");
    assert_eq!(active.title, "Single task");
    assert_eq!(active.verification_cmd, Some("cargo test".to_string()));
    assert_eq!(active.files_modify, vec![PathBuf::from("src/single.rs")]);
    assert!(active.depends_on.is_empty());

    // Single milestone DAG has no remaining pending tasks
    assert!(state.pending_tasks.is_empty());
    assert!(state.completed_tasks.is_empty());
}

#[test]
fn test_milestone_spec_and_execution_state_serde() {
    // Verify MilestoneSpec defaults the files array to an empty vector when omitted from JSON.
    // This allows lean milestone inputs from CLI/prompts without requiring explicit files lists.
    let json_no_files = r#"{"title":"Setup project","verify_cmd":"cargo check"}"#;
    let milestone: MilestoneSpec = serde_json::from_str(json_no_files)
        .expect("MilestoneSpec deserialization without files must succeed");
    assert_eq!(milestone.title, "Setup project");
    assert_eq!(milestone.verify_cmd, "cargo check");
    assert!(milestone.files.is_empty());

    // Verify roundtrip serialization of MilestoneSpec preserves all fields including files
    let milestone_with_files = MilestoneSpec {
        title: "Test with files".to_string(),
        verify_cmd: "cargo test".to_string(),
        files: vec![PathBuf::from("crates/core/src/lib.rs")],
        ..Default::default()
    };
    let serialized_milestone = serde_json::to_string(&milestone_with_files)
        .expect("MilestoneSpec serialization must succeed");
    let deserialized_milestone: MilestoneSpec = serde_json::from_str(&serialized_milestone)
        .expect("MilestoneSpec roundtrip deserialization must succeed");
    assert_eq!(deserialized_milestone, milestone_with_files);

    // Verify ExecutionState serialization includes base_branch when present
    let state_with_branch = ExecutionState {
        base_branch: Some("feat/direct-handoff".to_string()),
        ..Default::default()
    };
    let json_with_branch = serde_json::to_string(&state_with_branch)
        .expect("ExecutionState serialization with base_branch must succeed");
    assert!(json_with_branch.contains(r#""base_branch":"feat/direct-handoff""#));

    // Verify ExecutionState roundtrip deserialization preserves base_branch
    let deserialized_with_branch: ExecutionState = serde_json::from_str(&json_with_branch)
        .expect("ExecutionState deserialization with base_branch must succeed");
    assert_eq!(
        deserialized_with_branch.base_branch,
        Some("feat/direct-handoff".to_string())
    );

    // Verify ExecutionState deserialization defaults base_branch to None when omitted
    let minimal_json = r#"{"status":"not_started","step":0}"#;
    let deserialized_minimal: ExecutionState = serde_json::from_str(minimal_json)
        .expect("ExecutionState deserialization without base_branch must succeed");
    assert_eq!(deserialized_minimal.base_branch, None);

    // Verify ExecutionState serialization omits base_branch when None (skip_serializing_if)
    let state_no_branch = ExecutionState::default();
    let json_no_branch = serde_json::to_string(&state_no_branch)
        .expect("ExecutionState serialization without base_branch must succeed");
    assert!(!json_no_branch.contains("base_branch"));

    // Verify ExecutionState serialization includes goal when present
    let state_with_goal = ExecutionState {
        goal: Some("Build compiler".to_string()),
        ..Default::default()
    };
    let json_with_goal = serde_json::to_string(&state_with_goal)
        .expect("ExecutionState serialization with goal must succeed");
    assert!(json_with_goal.contains(r#""goal":"Build compiler""#));

    // Verify ExecutionState roundtrip deserialization preserves goal
    let deserialized_with_goal: ExecutionState = serde_json::from_str(&json_with_goal)
        .expect("ExecutionState deserialization with goal must succeed");
    assert_eq!(
        deserialized_with_goal.goal,
        Some("Build compiler".to_string())
    );

    // Verify ExecutionState deserialization defaults goal to None when omitted
    assert_eq!(deserialized_minimal.goal, None);

    // Verify ExecutionState serialization omits goal when None (skip_serializing_if)
    let state_no_goal = ExecutionState::default();
    let json_no_goal = serde_json::to_string(&state_no_goal)
        .expect("ExecutionState serialization without goal must succeed");
    assert!(!json_no_goal.contains("goal"));
}
