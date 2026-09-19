use std::fs;
use std::path::PathBuf;
use std::process::Command;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::{ExecutionStatus, MilestoneSpec};
use superstate_core::state_machine::{init_state_milestones, load_state, save_state_atomic};

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
    let _ = Command::new("git")
        .args(["config", "commit.gpgsign", "false"])
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
fn test_daemon_executes_sequential_milestones_with_checkpoints() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let milestones = vec![
        MilestoneSpec {
            id: Some("task-1".to_string()),
            title: "Create A".to_string(),
            verify_cmd: "test -f a.txt".to_string(),
            files: vec![PathBuf::from("a.txt")],
            ..Default::default()
        },
        MilestoneSpec {
            id: Some("task-2".to_string()),
            title: "Create B".to_string(),
            verify_cmd: "test -f b.txt".to_string(),
            files: vec![PathBuf::from("b.txt")],
            ..Default::default()
        },
    ];

    let state = init_state_milestones(Some("Sequential checkpoints"), milestones, None, true)
        .expect("init state");
    save_state_atomic(&state_path, &state).expect("save state");

    // Mock driver creates a.txt for task-1 and b.txt for task-2 based on SUPERSTATE_ACTIVE_TASK
    let driver_script = r#"
        if [ "$SUPERSTATE_ACTIVE_TASK" = "task-1" ]; then
            echo "alpha" > a.txt
        elif [ "$SUPERSTATE_ACTIVE_TASK" = "task-2" ]; then
            echo "beta" > b.txt
        fi
    "#;

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
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let report = runner.run_to_completion().expect("run_to_completion");

    assert_eq!(report.status, ExecutionStatus::Completed);
    assert_eq!(report.milestones_completed, 2);

    // Verify both files exist on disk
    assert!(repo_root.join("a.txt").exists(), "a.txt must exist");
    assert!(repo_root.join("b.txt").exists(), "b.txt must exist");

    // Verify git log contains checkpoint commits for both task-1 and task-2
    let git_log = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(repo_root)
        .output()
        .expect("git log");
    let log_str = String::from_utf8_lossy(&git_log.stdout);
    assert!(
        log_str.contains("checkpoint completed milestone task-1"),
        "git log must contain checkpoint commit for task-1, got: {}",
        log_str
    );
    assert!(
        log_str.contains("checkpoint completed milestone task-2"),
        "git log must contain checkpoint commit for task-2, got: {}",
        log_str
    );
}

#[test]
fn test_daemon_rollback_isolation_preserves_earlier_milestone_commits() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let milestones = vec![
        MilestoneSpec {
            id: Some("task-1".to_string()),
            title: "Task 1".to_string(),
            verify_cmd: "test -f a.txt".to_string(),
            files: vec![PathBuf::from("a.txt")],
            ..Default::default()
        },
        MilestoneSpec {
            id: Some("task-2".to_string()),
            title: "Task 2".to_string(),
            verify_cmd: "test -f b.txt".to_string(),
            files: vec![PathBuf::from("b.txt")],
            ..Default::default()
        },
    ];

    let state = init_state_milestones(Some("Rollback isolation"), milestones, None, true)
        .expect("init state");
    save_state_atomic(&state_path, &state).expect("save state");

    // Track attempts outside the git repo so git clean does not erase the counter
    let marker_dir = tempfile::tempdir().expect("marker dir");
    let marker_file = marker_dir.path().join("attempt_marker");
    let marker_file_str = marker_file.to_str().expect("valid path");

    // Driver: creates a.txt for task-1.
    // For task-2 attempt 1 (marker does not exist): creates marker and bad.txt (fails verification).
    // For task-2 attempt 2 (marker exists): creates b.txt (passes verification).
    let driver_script = format!(
        r#"
        if [ "$SUPERSTATE_ACTIVE_TASK" = "task-1" ]; then
            echo "a_content" > a.txt
        elif [ ! -f "{}" ]; then
            touch "{}"
            echo "corrupt" > bad.txt
        else
            echo "b_content" > b.txt
        fi
        "#,
        marker_file_str, marker_file_str
    );

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");

    // Step 1: Task 1 succeeds
    let outcome1 = runner.run_step().expect("step 1");
    assert!(outcome1.is_advanced());
    assert!(repo_root.join("a.txt").exists(), "a.txt created in task-1");

    // Step 2: Task 2 Attempt 1 writes bad.txt and fails verification, triggering auto-rollback
    let outcome2 = runner.run_step().expect("step 2");
    assert!(
        outcome2.is_failed(),
        "step 2 must fail verification and trigger rollback, got {:?}",
        outcome2
    );

    // Verify rollback isolation: a.txt is STILL present in repo, while bad.txt is removed
    assert!(
        repo_root.join("a.txt").exists(),
        "a.txt must still be present after task-2 rollback"
    );
    assert!(
        !repo_root.join("bad.txt").exists(),
        "bad.txt must be removed by rollback"
    );

    // Step 3: Task 2 Attempt 2 writes b.txt and passes verification
    let outcome3 = runner.run_step().expect("step 3");
    assert!(
        outcome3.is_advanced(),
        "step 3 must advance task-2, got {:?}",
        outcome3
    );

    // Final state verification
    assert!(repo_root.join("a.txt").exists(), "a.txt exists at end");
    assert!(repo_root.join("b.txt").exists(), "b.txt exists at end");
    assert!(!repo_root.join("bad.txt").exists(), "bad.txt does not exist at end");

    let final_state = load_state(&state_path).expect("load final state");
    assert_eq!(final_state.status, ExecutionStatus::Completed);
    assert_eq!(final_state.completed_tasks.len(), 2);
}

#[test]
fn test_daemon_deadlock_halts_with_blocked_outcome() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let state_dir = temp_dir.path().join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let milestones = vec![
        MilestoneSpec {
            id: Some("task-1".to_string()),
            title: "Task 1".to_string(),
            verify_cmd: "true".to_string(),
            depends_on: Some(vec![]),
            ..Default::default()
        },
        MilestoneSpec {
            id: Some("task-2".to_string()),
            title: "Task 2".to_string(),
            verify_cmd: "true".to_string(),
            depends_on: Some(vec!["task-99".to_string()]),
            ..Default::default()
        },
    ];

    let state = init_state_milestones(Some("Deadlock test"), milestones, None, true)
        .expect("init state");
    save_state_atomic(&state_path, &state).expect("save state");

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");

    // Run step on task-1: verification passes, but advance_step detects deadlock with task-2
    let outcome = runner.run_step().expect("run_step should return Ok(Blocked)");

    // Must return DaemonStepOutcome::Blocked
    match outcome {
        DaemonStepOutcome::Blocked { task_id, reason } => {
            assert_eq!(task_id, "task-1");
            assert!(
                reason.contains("pending tasks exist but none have satisfied dependencies"),
                "reason should mention unsatisfied dependencies, got: {}",
                reason
            );
        }
        other => panic!("expected DaemonStepOutcome::Blocked, got {:?}", other),
    }

    // Verify state was transitioned to Blocked and saved
    let loaded_state = load_state(&state_path).expect("load state");
    assert_eq!(loaded_state.status, ExecutionStatus::Blocked);

    // Verify [dag-blocked] note was recorded
    let dag_blocked_note = loaded_state
        .notes
        .iter()
        .find(|n| n.note.contains("[dag-blocked]"));
    assert!(
        dag_blocked_note.is_some(),
        "state notes must contain [dag-blocked] note"
    );
    let note_content = &dag_blocked_note.unwrap().note;
    assert!(
        note_content.contains("Milestone task-1 completed, but remaining tasks cannot advance"),
        "note must format correctly: {}",
        note_content
    );
}
