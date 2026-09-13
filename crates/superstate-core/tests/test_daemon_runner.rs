use std::path::PathBuf;
use superstate_core::daemon::{DaemonConfig, DaemonError, DaemonRunner, DaemonStepOutcome};

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

#[test]
fn test_daemon_step_advancer_invoked() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Advancer Milestone".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
    }];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker output'".to_string()),
        max_retries: 3,
        timeout_secs: 10,
    };

    use std::sync::{Arc, Mutex};
    let invoked_path = Arc::new(Mutex::new(None));
    let invoked_clone = invoked_path.clone();

    let advancer = Box::new(move |p: &std::path::Path| {
        let mut guard = invoked_clone.lock().unwrap();
        *guard = Some(p.to_path_buf());
        Ok(())
    });

    let mut runner = DaemonRunner::new(config)
        .unwrap()
        .with_step_advancer(advancer);

    let outcome = runner.run_step().unwrap();
    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    let recorded = invoked_path.lock().unwrap().take();
    assert_eq!(recorded, Some(state_path));
}

#[test]
fn test_daemon_step_advancer_error_propagation() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Advancer Failure Milestone".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
    }];

    // Initialize state with in-place execution mode
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker output'".to_string()),
        max_retries: 3,
        timeout_secs: 10,
    };

    // Configure a StepAdvancer that simulates downstream failure during advancement
    let advancer = Box::new(|_p: &std::path::Path| Err("custom step advancer error".to_string()));

    let mut runner = DaemonRunner::new(config)
        .unwrap()
        .with_step_advancer(advancer);

    // Verify error is converted into DaemonError::StepAdvancer and propagated to caller
    let err = runner.run_step().unwrap_err();
    match err {
        DaemonError::StepAdvancer(msg) => {
            assert_eq!(msg, "custom step advancer error");
        }
        other => panic!("Expected DaemonError::StepAdvancer, got: {:?}", other),
    }
}

#[test]
fn test_daemon_driver_rescued_stderr_bounded_to_1000_chars() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let target_file = dir.path().join("bounded_rescued.txt");
    let target_str = target_file.to_str().unwrap();

    let milestones = vec![MilestoneSpec {
        title: "Rescue Milestone Bounded Stderr".to_string(),
        verify_cmd: format!("test -f {}", target_str),
        files: vec![],
    }];

    let state = init_state_milestones(Some("Rescue Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Driver creates target file, writes 1500 chars to stderr, and exits with code 1
    let driver_cmd = format!(
        "touch {} && printf '%1500s' | tr ' ' 'E' >&2 && exit 1",
        target_str
    );

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_cmd),
        max_retries: 3,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.run_step().unwrap();

    // Verification gate passes despite driver non-zero exit code
    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    // Verify warning entry in state notes is bounded to 1000 characters
    let updated_state = load_state(&state_path).unwrap();
    let warn_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[driver-warn]"))
        .expect("Expected [driver-warn] note in state.notes");

    let prefix = "[driver-warn] Driver for task task-1 exited with code 1: ";
    assert!(
        warn_note.note.starts_with(prefix),
        "Expected note to start with prefix '{}', found: {}",
        prefix,
        warn_note.note
    );
    let captured_stderr = &warn_note.note[prefix.len()..];
    assert_eq!(
        captured_stderr.chars().count(),
        1000,
        "Expected captured stderr to be bounded to exactly 1000 chars, got length {}",
        captured_stderr.chars().count()
    );
}

#[test]
fn test_daemon_driver_rescue_on_nonzero_exit() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let target_file = dir.path().join("rescued.txt");
    let target_str = target_file.to_str().unwrap();

    let milestones = vec![MilestoneSpec {
        title: "Rescue Milestone".to_string(),
        verify_cmd: format!("test -f {}", target_str),
        files: vec![],
    }];

    let state = init_state_milestones(Some("Rescue Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Driver creates the file but exits with code 1 (simulating tooling error)
    let driver_cmd = format!("touch {} && exit 1", target_str);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_cmd),
        max_retries: 3,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.run_step().unwrap();

    // Gate passes despite driver exit code 1
    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    // Check that [driver-warn] was recorded in state notes
    let updated_state = load_state(&state_path).unwrap();
    let has_driver_warn = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[driver-warn]"));
    assert!(
        has_driver_warn,
        "Expected [driver-warn] note in state.notes, found: {:?}",
        updated_state.notes
    );
}

#[test]
fn test_daemon_rich_retry_prompt_on_failure() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Failing Milestone".to_string(),
        verify_cmd: "sh -c 'echo \"syntax error in main.rs line 42\" >&2 && exit 1'".to_string(),
        files: vec![],
    }];

    let state = init_state_milestones(Some("Retry Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Test driver captures stdin into captured_stdin.txt
    let captured_prompt_file = dir.path().join("captured_stdin.txt");
    let captured_str = captured_prompt_file.to_str().unwrap();
    let driver_cmd = format!("cat > {}", captured_str);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_cmd),
        max_retries: 3,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();

    // First attempt fails verification
    let outcome1 = runner.run_step().unwrap();
    assert!(matches!(outcome1, DaemonStepOutcome::Failed { .. }));

    // Second attempt should receive the rich retry prompt
    let outcome2 = runner.run_step().unwrap();
    assert!(matches!(outcome2, DaemonStepOutcome::Failed { .. }));

    let captured = std::fs::read_to_string(&captured_prompt_file).unwrap();
    assert!(
        captured.contains("## 🚨 PREVIOUS ATTEMPT VERIFICATION FAILURE"),
        "Expected previous failure header in retry prompt, got:\n{}",
        captured
    );
    assert!(
        captured.contains("syntax error in main.rs line 42"),
        "Expected compiler/test stderr in retry prompt, got:\n{}",
        captured
    );

    // Verify prompt file was written to .agent-state/prompts/default.md
    let prompt_file = dir.path().join(".agent-state/prompts/default.md");
    assert!(
        prompt_file.exists(),
        "Prompt file must exist at .agent-state/prompts/default.md"
    );
    let prompt_content = std::fs::read_to_string(&prompt_file).unwrap();
    assert!(prompt_content.contains("## 🚨 PREVIOUS ATTEMPT VERIFICATION FAILURE"));
}

#[test]
fn test_daemon_auto_commit_on_pass_in_place() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();
    let state_path = repo_dir.join(".agent-state/state.json");

    // Initialize a git repo in dir.path()
    use std::process::Command;
    Command::new("git")
        .args(["init"])
        .current_dir(repo_dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test Runner"])
        .current_dir(repo_dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_dir)
        .output()
        .unwrap();

    // Create an initial commit so repo has HEAD
    std::fs::write(repo_dir.join("init.txt"), "init").unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo_dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(repo_dir)
        .output()
        .unwrap();

    let milestones = vec![MilestoneSpec {
        title: "Create Feature".to_string(),
        verify_cmd: "test -f feature.txt".to_string(),
        files: vec![],
    }];

    // Note: 4th arg is `no_worktree: true` -> in-place mode
    let state = init_state_milestones(Some("Auto Commit Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'done' > feature.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.run_step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    // Verify git log contains the checkpoint commit
    let log_out = Command::new("git")
        .args(["log", "-1", "--pretty=%B"])
        .current_dir(repo_dir)
        .output()
        .unwrap();
    let commit_msg = String::from_utf8_lossy(&log_out.stdout);
    assert!(
        commit_msg
            .contains("chore(superstate): checkpoint completed milestone task-1: Create Feature"),
        "Expected checkpoint commit message, got:\n{}",
        commit_msg
    );
}
