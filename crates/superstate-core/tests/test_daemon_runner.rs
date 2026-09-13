use std::path::PathBuf;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};

use superstate_core::models::{ExecutionStatus, MilestoneSpec};
use superstate_core::state_machine::{init_state_milestones, load_state, save_state_atomic};
use tempfile::tempdir;

#[test]
fn test_daemon_run_step_success() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![
        MilestoneSpec {
            title: "Milestone 1".to_string(),
            verify_cmd: "true".to_string(),
            files: vec![PathBuf::from("file1.txt")],
        },
        MilestoneSpec {
            title: "Milestone 2".to_string(),
            verify_cmd: "true".to_string(),
            files: vec![PathBuf::from("file2.txt")],
        },
    ];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker output'".to_string()),
        max_retries: 3,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.run_step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    let updated_state = load_state(&state_path).unwrap();
    assert_eq!(updated_state.step, 2);
    assert_eq!(updated_state.completed_tasks.len(), 1);
    assert_eq!(updated_state.completed_tasks[0].id, "task-1");
    assert_eq!(updated_state.active_task.as_ref().unwrap().id, "task-2");
}

#[test]
fn test_daemon_run_step_verification_failure() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Failing Milestone".to_string(),
        verify_cmd: "false".to_string(),
        files: vec![],
    }];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker trying'".to_string()),
        max_retries: 2,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.run_step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Failed { .. }));

    let updated_state = load_state(&state_path).unwrap();
    assert_eq!(updated_state.step, 1);
    assert_eq!(updated_state.completed_tasks.len(), 0);
    assert_eq!(updated_state.active_task.as_ref().unwrap().id, "task-1");
    assert!(!updated_state.notes.is_empty());
}

#[test]
fn test_daemon_run_to_completion() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![
        MilestoneSpec {
            title: "Task A".to_string(),
            verify_cmd: "true".to_string(),
            files: vec![],
        },
        MilestoneSpec {
            title: "Task B".to_string(),
            verify_cmd: "true".to_string(),
            files: vec![],
        },
    ];

    let state = init_state_milestones(Some("Complete Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker completed'".to_string()),
        max_retries: 3,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let report = runner.run_to_completion().unwrap();

    assert_eq!(report.milestones_completed, 2);
    assert_eq!(report.status, ExecutionStatus::Completed);

    let final_state = load_state(&state_path).unwrap();
    assert_eq!(final_state.status, ExecutionStatus::Completed);
    assert_eq!(final_state.completed_tasks.len(), 2);
    assert!(final_state.active_task.is_none());

    let completion_file = state_path.parent().unwrap().join("completion.json");
    assert!(completion_file.exists());
}
