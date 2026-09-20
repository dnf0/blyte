use std::fs;
use std::process::Command;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{load_state, save_state_atomic};

/// Helper to configure a temporary git repository rooted on the main branch.
fn setup_temp_git_repo(path: &std::path::Path) -> String {
    let init_res = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output();
    if let Ok(out) = init_res {
        if !out.status.success() {
            let _ = Command::new("git").arg("init").current_dir(path).output();
            let _ = Command::new("git")
                .args(["checkout", "-b", "main"])
                .current_dir(path)
                .output();
        }
    }

    let _ = Command::new("git")
        .args(["config", "user.name", "Superstate Test"])
        .current_dir(path)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@superstate.local"])
        .current_dir(path)
        .output();

    // Create an initial tracked file and commit
    fs::write(path.join("tracked.txt"), "initial content\n").expect("write tracked.txt");
    let _ = Command::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(path)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(path)
        .output();

    let head_out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .expect("git rev-parse HEAD");
    String::from_utf8_lossy(&head_out.stdout).trim().to_string()
}

#[test]
fn test_daemon_rollback_on_verification_failure() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(
        !initial_commit.is_empty(),
        "initial commit must not be empty"
    );

    // Initialize execution state targeting in-place repository
    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-rollback-1".to_string(),
            title: "Task with failing verification requiring rollback".to_string(),
            // Verification command fails
            verification_cmd: Some("test -f milestone_done.txt".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test rollback behavior on verification failure".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).expect("save state");

    // Driver command: modifies tracked.txt, creates untracked.txt and untracked_dir/nested.txt
    let driver_script = "echo 'corrupted driver content' > tracked.txt && \
                         echo 'untracked artifact' > untracked.txt && \
                         mkdir -p untracked_dir && \
                         echo 'nested' > untracked_dir/nested.txt";

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script.to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
        shunt_driver_output: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.run_step().expect("run_step");

    // 1. Outcome must be Failed (retries remaining)
    assert!(
        matches!(outcome, DaemonStepOutcome::Failed { .. }),
        "expected Failed outcome, got {:?}",
        outcome
    );

    // 2. Tracked file must be restored to initial commit content
    let tracked_content =
        fs::read_to_string(repo_root.join("tracked.txt")).expect("read tracked.txt");
    assert_eq!(
        tracked_content, "initial content\n",
        "tracked file should be rolled back to initial commit content"
    );

    // 3. Untracked file and directory must be cleaned (-fd)
    assert!(
        !repo_root.join("untracked.txt").exists(),
        "untracked.txt should be removed by git clean -fd"
    );
    assert!(
        !repo_root.join("untracked_dir").exists(),
        "untracked_dir should be removed by git clean -fd"
    );

    // 4. Epistemic note must be recorded
    let updated_state = load_state(&state_path).expect("load updated state");
    let rollback_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[rollback]"));
    assert!(
        rollback_note.is_some(),
        "state.notes must contain a [rollback] note"
    );
    let note_content = &rollback_note.unwrap().note;
    assert!(
        note_content.contains("Task task-rollback-1: attempt 1 failed verification"),
        "note should reference task id and attempt number: {}",
        note_content
    );
    assert!(
        note_content.contains(&format!("baseline commit {}", initial_commit)),
        "note should reference baseline commit SHA: {}",
        note_content
    );
}

#[test]
fn test_daemon_no_rollback_when_disabled() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let _ = setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-no-rollback".to_string(),
            title: "Task without rollback".to_string(),
            verification_cmd: Some("test -f milestone_done.txt".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test non-rollback behavior".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).expect("save state");

    let driver_script = "echo 'corrupted driver content' > tracked.txt && \
                         echo 'untracked artifact' > untracked.txt";

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script.to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.run_step().expect("run_step");

    assert!(matches!(outcome, DaemonStepOutcome::Failed { .. }));

    // When disabled, dirty tree is preserved
    let tracked_content =
        fs::read_to_string(repo_root.join("tracked.txt")).expect("read tracked.txt");
    assert_eq!(tracked_content.trim(), "corrupted driver content");
    assert!(repo_root.join("untracked.txt").exists());

    let updated_state = load_state(&state_path).expect("load updated state");
    let rollback_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[rollback]"));
    assert!(
        rollback_note.is_none(),
        "state.notes should NOT contain a [rollback] note when auto_rollback is false"
    );
}

#[test]
fn test_daemon_config_default_has_auto_rollback_enabled() {
    let config = DaemonConfig::default();
    assert!(
        config.auto_rollback,
        "DaemonConfig::default().auto_rollback must be true"
    );
}

#[test]
fn test_daemon_deserialization_defaults_auto_rollback_to_true() {
    let json_str =
        r#"{"state_path": ".agent-state/state.json", "max_retries": 3, "timeout_secs": 120}"#;
    let config: DaemonConfig = serde_json::from_str(json_str).expect("deserialize config");
    assert!(
        config.auto_rollback,
        "deserialized DaemonConfig without auto_rollback field must default to true"
    );
}
