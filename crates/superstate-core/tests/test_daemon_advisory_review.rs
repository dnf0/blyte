use std::fs;
use std::process::Command;
use tempfile::{tempdir, TempDir};

use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::MilestoneSpec;
use superstate_core::state_machine::{init_state_milestones, load_state, save_state_atomic};

fn setup_test_git_repo() -> TempDir {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo)
        .output()
        .unwrap();

    fs::write(repo.join("README.md"), "# Initial content\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(repo)
        .output()
        .unwrap();

    dir
}

#[test]
fn test_daemon_config_default_advisory_review_false() {
    let config = DaemonConfig::default();
    assert!(!config.advisory_review);
}

#[test]
fn test_daemon_advisory_review_clean_diff_advances() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Clean Milestone".to_string(),
        verify_cmd: "test -f README.md".to_string(),
        files: vec![],
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    assert!(
        matches!(outcome, DaemonStepOutcome::Advanced { .. }),
        "Milestone on clean diff should advance without failure, got: {:?}",
        outcome
    );

    let updated_state = load_state(&state_path).unwrap();
    assert!(
        !updated_state
            .notes
            .iter()
            .any(|n| n.note.contains("[review-advisory]")),
        "Clean diff must not produce advisory review notes"
    );
}

#[test]
fn test_daemon_advisory_review_blocking_issues_vacuous_gate_fails() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Vacuous Gate Milestone".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Modify a tracked file so diff exists
    fs::write(repo.join("README.md"), "# Flawed code\n").unwrap();

    // Mock driver that outputs a ReviewReport JSON with a Critical blocking finding
    let mock_report_json = r#"{"summary":"Critical SQL injection detected","findings":[{"severity":"Critical","file":"README.md","line_range":[1,1],"problem":"Dangerous flaw","suggested_fix":"Sanitize input"}],"has_blocking_issues":true}"#;
    let mock_driver = format!("echo '{}'", mock_report_json);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    match outcome {
        DaemonStepOutcome::Failed { task_id, reason, .. } => {
            assert_eq!(task_id, "task-1");
            assert!(
                reason.contains("advisory-review: blocking issues detected on vacuous gate"),
                "Expected advisory-review failure reason, got: {}",
                reason
            );
        }
        other => panic!("Expected DaemonStepOutcome::Failed, got: {:?}", other),
    }

    let updated_state = load_state(&state_path).unwrap();
    let has_advisory_note = updated_state.notes.iter().any(|n| {
        n.note.contains("[review-advisory]")
            && n.note.contains("Dangerous flaw")
            && n.note.contains("Task task-1")
    });
    assert!(
        has_advisory_note,
        "Expected [review-advisory] note in ledger notes, notes were: {:?}",
        updated_state.notes
    );

    // Milestone must not have advanced
    assert_eq!(updated_state.step, 1);
    assert_eq!(updated_state.completed_tasks.len(), 0);
}

#[test]
fn test_daemon_advisory_review_blocking_issues_non_vacuous_gate_advances() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Strict Gate Milestone".to_string(),
        verify_cmd: "echo gate_ok".to_string(),
        files: vec![],
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Modify a tracked file so diff exists
    fs::write(repo.join("README.md"), "# Modified code\n").unwrap();

    let mock_report_json = r#"{"summary":"Critical flaw detected by review","findings":[{"severity":"Critical","file":"README.md","line_range":[1,1],"problem":"Potential bug","suggested_fix":"Fix logic"}],"has_blocking_issues":true}"#;
    let mock_driver = format!("echo '{}'", mock_report_json);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    assert!(
        matches!(outcome, DaemonStepOutcome::Advanced { .. }),
        "Non-vacuous gate milestone must advance even with advisory findings, got: {:?}",
        outcome
    );

    let updated_state = load_state(&state_path).unwrap();
    let has_advisory_note = updated_state.notes.iter().any(|n| {
        n.note.contains("[review-advisory]")
            && n.note.contains("Potential bug")
            && n.note.contains("Task task-1")
    });
    assert!(
        has_advisory_note,
        "Expected [review-advisory] note in ledger notes for diagnostic context, notes were: {:?}",
        updated_state.notes
    );

    // Milestone completed and advanced
    assert_eq!(updated_state.completed_tasks.len(), 1);
}
