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
        .args(["commit", "-m", "initial commit", "--no-gpg-sign"])
        .current_dir(repo)
        .output()
        .unwrap();

    dir
}

#[test]
fn test_daemon_config_default_review_blocking_false() {
    let config = DaemonConfig::default();
    assert!(!config.review_blocking);
}

#[test]
fn test_daemon_blocking_review_fails_on_blocking_issues() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Strict Gate Milestone".to_string(),
        verify_cmd: "test -f README.md".to_string(),
        files: vec![],
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Modify a tracked file and commit it, simulating driver committing changes
    fs::write(repo.join("README.md"), "# Broken code with security flaw\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "commit",
            "-m",
            "feat: implement flawed feature",
            "--no-gpg-sign",
        ])
        .current_dir(repo)
        .output()
        .unwrap();

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
        review_blocking: true,
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
                reason.contains("code-review"),
                "Reason should cite code-review, got: {}",
                reason
            );
        }
        other => panic!(
            "Expected DaemonStepOutcome::Failed with review_blocking=true, got: {:?}",
            other
        ),
    }

    let updated_state = load_state(&state_path).unwrap();
    // Milestone must not have advanced
    assert_eq!(updated_state.step, 1);
    assert_eq!(updated_state.completed_tasks.len(), 0);
    assert!(
        updated_state
            .notes
            .iter()
            .any(|n| n.note.contains("Dangerous flaw")),
        "Expected review findings recorded in notes"
    );
}

#[test]
fn test_daemon_blocking_review_clean_report_advances() {
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

    fs::write(repo.join("README.md"), "# Clean valid code\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "commit",
            "-m",
            "feat: clean implementation",
            "--no-gpg-sign",
        ])
        .current_dir(repo)
        .output()
        .unwrap();

    // Mock driver returns clean report
    let mock_report_json =
        r#"{"summary":"Code is clean and correct","findings":[],"has_blocking_issues":false}"#;
    let mock_driver = format!("echo '{}'", mock_report_json);

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
        review_blocking: true,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();

    assert!(
        matches!(outcome, DaemonStepOutcome::Advanced { .. }),
        "Clean review with review_blocking must advance milestone, got: {:?}",
        outcome
    );

    let updated_state = load_state(&state_path).unwrap();
    assert_eq!(updated_state.completed_tasks.len(), 1);
}

#[test]
fn test_daemon_advisory_review_without_blocking_advances_on_non_vacuous_gate() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Non Vacuous Gate Milestone".to_string(),
        verify_cmd: "test -f README.md".to_string(),
        files: vec![],
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    fs::write(repo.join("README.md"), "# Flawed code\n").unwrap();

    // Mock driver returns blocking issues
    let mock_report_json = r#"{"summary":"Flaws found","findings":[{"severity":"High","file":"README.md","line_range":[1,1],"problem":"Advisory issue","suggested_fix":"Fix later"}],"has_blocking_issues":true}"#;
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
        "When review_blocking is false, non-vacuous gate milestone must advance, got: {:?}",
        outcome
    );

    let updated_state = load_state(&state_path).unwrap();
    assert_eq!(updated_state.completed_tasks.len(), 1);
    assert!(
        updated_state
            .notes
            .iter()
            .any(|n| n.note.contains("Advisory issue")),
        "Advisory notes must be recorded"
    );
}

#[test]
fn test_daemon_tracks_milestone_start_commit_across_retries() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let initial_head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let milestones = vec![MilestoneSpec {
        title: "Milestone With Commits".to_string(),
        verify_cmd: "test -f README.md".to_string(),
        files: vec![],
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let script = repo.join("driver.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
echo "committed change" >> README.md
git add README.md >/dev/null 2>&1
git commit -m "commit in milestone" --no-gpg-sign >/dev/null 2>&1 || true
cat << 'EOF'
{"summary":"Commit flaw","findings":[{"severity":"High","file":"README.md","line_range":[1,1],"problem":"Bad commit","suggested_fix":"Undo"}],"has_blocking_issues":true}
EOF
"#,
    )
    .unwrap();
    Command::new("chmod")
        .args(["+x", script.to_str().unwrap()])
        .output()
        .unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(script.to_str().unwrap().to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
        review_blocking: true,
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
            assert!(reason.contains("code-review"));
        }
        other => panic!("Expected Failed, got {:?}", other),
    }

    assert_eq!(
        runner.milestone_start_commits().get("task-1"),
        Some(&initial_head),
        "milestone_start_commits should preserve the commit from before driver executed"
    );
}
