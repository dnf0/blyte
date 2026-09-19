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
        ..Default::default()
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
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
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
        ..Default::default()
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
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    match outcome {
        DaemonStepOutcome::Failed {
            task_id, reason, ..
        } => {
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
            && n.note.contains("Dangerous flaw (fix: Sanitize input)")
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
        ..Default::default()
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
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
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
            && n.note.contains("Potential bug (fix: Fix logic)")
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

#[test]
fn test_daemon_advisory_review_prioritizes_critical_over_low_and_includes_fix() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Secure Auth Handler".to_string(),
        verify_cmd: "echo gate_ok".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    fs::write(repo.join("README.md"), "# Updated auth logic\n").unwrap();

    let mock_report_json = r#"{"summary":"Security and formatting issues detected","findings":[{"severity":"Low","file":"README.md","line_range":[1,1],"problem":"Trailing whitespace","suggested_fix":"Trim line"},{"severity":"Critical","file":"README.md","line_range":[2,5],"problem":"Unsanitized input used in SQL query","suggested_fix":"Use parameterized statements"}],"has_blocking_issues":true}"#;
    let mock_driver = format!("echo '{}'", mock_report_json);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    let updated_state = load_state(&state_path).unwrap();
    let advisory_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[review-advisory]"))
        .expect("Expected [review-advisory] note in ledger");

    assert!(
        advisory_note
            .note
            .contains("Unsanitized input used in SQL query (fix: Use parameterized statements)"),
        "Expected critical finding with suggested fix to be selected, got: {}",
        advisory_note.note
    );
    assert!(
        !advisory_note.note.contains("Trailing whitespace"),
        "Low severity finding should not be selected when Critical finding exists, got: {}",
        advisory_note.note
    );
}

#[test]
fn test_daemon_advisory_review_aggregates_multiple_blocking_issues() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Multi Issue Milestone".to_string(),
        verify_cmd: "echo ok".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    fs::write(repo.join("README.md"), "# Updated code\n").unwrap();

    let mock_report_json = r#"{"summary":"Multiple security issues detected","findings":[{"severity":"Critical","file":"src/auth.rs","line_range":[10,12],"problem":"Hardcoded secret","suggested_fix":"Use env var"},{"severity":"High","file":"src/api.rs","line_range":[20,25],"problem":"Missing CSRF check","suggested_fix":"Add CSRF middleware"},{"severity":"High","file":"src/db.rs","line_range":[30,35],"problem":"Raw query used","suggested_fix":"Use ORM query"},{"severity":"Critical","file":"src/overflow.rs","line_range":[40,45],"problem":"Fourth issue beyond cap","suggested_fix":"Should be omitted"}],"has_blocking_issues":true}"#;
    let mock_driver = format!("echo '{}'", mock_report_json);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    let updated_state = load_state(&state_path).unwrap();
    let advisory_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[review-advisory]"))
        .expect("Expected [review-advisory] note in ledger");

    let expected_prefix = "[review-advisory] Task task-1: Multiple security issues detected. Issues: 1) src/auth.rs: Hardcoded secret (fix: Use env var); 2) src/api.rs: Missing CSRF check (fix: Add CSRF middleware); 3) src/db.rs: Raw query used (fix: Use ORM query)";
    assert_eq!(advisory_note.note, expected_prefix);
    assert!(!advisory_note.note.contains("Fourth issue beyond cap"));
}

#[test]
fn test_daemon_advisory_review_works_in_git_subdirectory() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let sub = repo.join("subproject");
    fs::create_dir_all(&sub).unwrap();
    let state_path = sub.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Subproject Milestone".to_string(),
        verify_cmd: "echo ok".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    fs::write(sub.join("sub.txt"), "sub content\n").unwrap();

    let mock_report_json = r#"{"summary":"Subproject review finding","findings":[{"severity":"High","file":"sub.txt","line_range":[1,1],"problem":"Missing header","suggested_fix":"Add header"}],"has_blocking_issues":true}"#;
    let mock_driver = format!("echo '{}'", mock_report_json);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Advanced { .. }));

    let updated_state = load_state(&state_path).unwrap();
    let advisory_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[review-advisory]"))
        .expect("Expected [review-advisory] note even when running in git subdirectory");

    assert!(advisory_note
        .note
        .contains("Issues: 1) sub.txt: Missing header (fix: Add header)"));
}

#[test]
fn test_is_inside_git_repo_paths() {
    use std::path::Path;
    use superstate_core::daemon::is_inside_git_repo;

    // A fresh non-git temporary directory
    let non_git_dir = tempfile::tempdir().unwrap();
    assert!(!is_inside_git_repo(non_git_dir.path()));

    // A git repository
    let git_dir = setup_test_git_repo();
    assert!(is_inside_git_repo(git_dir.path()));

    // A subdirectory inside git repository
    let sub = git_dir.path().join("sub/dir");
    fs::create_dir_all(&sub).unwrap();
    assert!(is_inside_git_repo(&sub));

    // Non-existent path in non-git directory
    assert!(!is_inside_git_repo(&non_git_dir.path().join("nonexistent")));

    // Empty path: does not panic or falsely assert
    let _ = is_inside_git_repo(Path::new(""));
}
