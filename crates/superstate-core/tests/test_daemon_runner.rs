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
            verify_cmd: "test -f worker-ran.txt".to_string(),
            files: vec![PathBuf::from("file1.txt")],
            ..Default::default()
        },
        MilestoneSpec {
            title: "Milestone 2".to_string(),
            verify_cmd: "test -f worker-ran.txt".to_string(),
            files: vec![PathBuf::from("file2.txt")],
            ..Default::default()
        },
    ];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("touch worker-ran.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
        ..Default::default()
    }];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker trying'".to_string()),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
            ..Default::default()
        },
        MilestoneSpec {
            title: "Task B".to_string(),
            verify_cmd: "true".to_string(),
            files: vec![],
            ..Default::default()
        },
    ];

    let state = init_state_milestones(Some("Complete Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker completed'".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
fn test_daemon_run_to_completion_branch_scoped_slug() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/branches/feat-alpha.json");

    let milestones = vec![MilestoneSpec {
        title: "Task Branch Alpha".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
        ..Default::default()
    }];

    let state = init_state_milestones(Some("Branch Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'worker completed'".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let report = runner.run_to_completion().unwrap();

    assert_eq!(report.milestones_completed, 1);
    assert_eq!(report.status, ExecutionStatus::Completed);

    let scoped_completion_file = state_path
        .parent()
        .unwrap()
        .join("completion-feat-alpha.json");
    assert!(
        scoped_completion_file.exists(),
        "completion-feat-alpha.json must exist"
    );
    assert_eq!(
        report.completion_file, scoped_completion_file,
        "report.completion_file must point to scoped completion file"
    );

    let legacy_completion_file = state_path.parent().unwrap().join("completion.json");
    assert!(
        !legacy_completion_file.exists(),
        "secondary completion.json must NOT exist for branch-scoped daemons to eliminate race conditions"
    );
}

#[test]
fn test_daemon_step_advancer_invoked() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![MilestoneSpec {
        title: "Advancer Milestone".to_string(),
        verify_cmd: "test -f worker-ran.txt".to_string(),
        files: vec![],
        ..Default::default()
    }];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("touch worker-ran.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
        verify_cmd: "test -f worker-ran.txt".to_string(),
        files: vec![],
        ..Default::default()
    }];

    // Initialize state with in-place execution mode
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("touch worker-ran.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
        ..Default::default()
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
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
        ..Default::default()
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
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
        ..Default::default()
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
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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
        ..Default::default()
    }];

    // Note: 4th arg is `no_worktree: true` -> in-place mode
    let state = init_state_milestones(Some("Auto Commit Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'done' > feature.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
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

/// A worker that keeps changing the tree must keep earning attempts.
///
/// `max_retries` counts consecutive attempts that change nothing. A fixed total cap
/// abandoned agents that were visibly converging while wall-clock budget remained --
/// the daemon gave up on a Cython build after 3 attempts having used 46% of its budget,
/// on a task the turn-based arm solved.
#[test]
fn test_progress_renews_the_retry_budget() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();

    let state_path = repo.join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Never satisfied".to_string(),
        verify_cmd: "false".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Each attempt writes a distinct file, so every attempt registers as progress.
    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("date +%s%N > progress-$$.txt".to_string()),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();

    // With a fixed cap this would block on attempt 2. Progress must carry it past that.
    for attempt in 1..=4 {
        let outcome = runner.run_step().unwrap();
        assert!(
            matches!(outcome, DaemonStepOutcome::Failed { .. }),
            "attempt {attempt} should keep going while the worker makes progress, got {outcome:?}"
        );
    }
}

/// A worker that changes nothing must still be cut off at `max_retries`.
#[test]
fn test_stalled_worker_still_blocks() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();

    let state_path = repo.join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Never satisfied".to_string(),
        verify_cmd: "false".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // A driver that touches nothing: every attempt is a stall.
    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("true".to_string()),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();

    assert!(matches!(
        runner.run_step().unwrap(),
        DaemonStepOutcome::Failed { .. }
    ));
    assert!(
        matches!(
            runner.run_step().unwrap(),
            DaemonStepOutcome::Blocked { .. }
        ),
        "a worker that changes nothing must still be cut off"
    );
}

/// Edits inside an untracked directory must count as progress.
///
/// Regression: the first progress-aware implementation asked git for the working-tree
/// state. Git collapses an untracked directory to a single `?? dir/` line regardless of
/// what changes inside it, and `git diff HEAD` covers neither untracked files nor a repo
/// with no HEAD. A worker editing a freshly cloned source tree therefore produced an
/// identical fingerprint every attempt and was blocked as stalled while progressing --
/// exactly what happened to build-cython-ext, whose whole job is to fix a cloned package.
#[test]
fn test_progress_inside_an_untracked_directory_counts() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();
    std::fs::create_dir_all(repo.join("vendored")).unwrap();

    let state_path = repo.join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Never satisfied".to_string(),
        verify_cmd: "false".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Writes only ever land inside the untracked `vendored/` directory, so `git status`
    // reports the same single line on every attempt.
    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("date +%s%N > vendored/edit-$$.txt".to_string()),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();

    for attempt in 1..=4 {
        let outcome = runner.run_step().unwrap();
        assert!(
            matches!(outcome, DaemonStepOutcome::Failed { .. }),
            "attempt {attempt}: edits inside an untracked directory are progress, got {outcome:?}"
        );
    }
}

/// The daemon's own `.agent-state` writes must never count as progress.
///
/// It rewrites prompts, logs and driver turns on every attempt. Counting those would
/// renew the retry budget forever and the daemon would never stop.
#[test]
fn test_daemon_own_state_writes_are_not_progress() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();

    let state_path = repo.join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Never satisfied".to_string(),
        verify_cmd: "false".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // The driver touches nothing outside .agent-state, which the daemon itself rewrites.
    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("true".to_string()),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();

    assert!(matches!(
        runner.run_step().unwrap(),
        DaemonStepOutcome::Failed { .. }
    ));
    assert!(
        matches!(
            runner.run_step().unwrap(),
            DaemonStepOutcome::Blocked { .. }
        ),
        "daemon-authored state must not renew the retry budget"
    );
}

/// A gate that cannot fail must not end the run after one turn.
///
/// Regression: with the harness supplying `verify_cmd = "true"` (every task whose real
/// verifier is sealed), the daemon ran one driver turn, saw an unconditional pass, recorded
/// completion and exited. It scored 50% against superstate_mcp's 70% on an identical
/// 20-task set while running FASTER than the unassisted baseline -- not working harder,
/// just exiting on a gate that could not say no. superstate_mcp's iteration is turn-driven
/// and degrades gracefully; the daemon's was gate-driven and did not.
#[test]
fn test_ungated_task_keeps_driving_past_the_first_pass() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();

    let state_path = repo.join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Ungated".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("date +%s%N > turn-$$.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();

    // The first two turns must NOT complete, despite `true` passing every time.
    for turn in 1..=2 {
        let outcome = runner.run_step().unwrap();
        assert!(
            matches!(outcome, DaemonStepOutcome::Failed { .. }),
            "turn {turn}: an unconditional pass must not be accepted as completion, got {outcome:?}"
        );
    }
    // The third satisfies the floor and is allowed to advance.
    assert!(
        matches!(
            runner.run_step().unwrap(),
            DaemonStepOutcome::Advanced { .. }
        ),
        "the turn floor should be satisfied by the third turn"
    );
}

/// A real contract still completes the moment it passes -- the floor must not prolong it.
#[test]
fn test_real_gate_completes_immediately() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .output()
        .unwrap();

    let state_path = repo.join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Gated".to_string(),
        verify_cmd: "test -f produced.txt".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("touch produced.txt".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();
    assert!(
        matches!(
            runner.run_step().unwrap(),
            DaemonStepOutcome::Advanced { .. }
        ),
        "a falsifiable gate that passes must complete on the first turn"
    );
}

/// The ungated turn floor must terminate when there is no driver to advance it.
///
/// Regression: the floor returned Failed before the retry accounting ran, so with
/// `driver_cmd: None` the turn counter never incremented, the floor was never reached, and
/// run_to_completion looped forever. It hung test_cli_daemon_lifecycle.
#[test]
fn test_ungated_floor_terminates_without_a_driver() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");
    let milestones = vec![MilestoneSpec {
        title: "Ungated, no driver".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 1,
        timeout_secs: 5,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();
    assert!(
        matches!(
            runner.run_step().unwrap(),
            DaemonStepOutcome::Advanced { .. }
        ),
        "with no driver the floor cannot apply, so the task must advance"
    );
}

/// Advisory gate mode mirrors superstate_mcp: bounded by turns, not by gate satisfaction.
///
/// superstate_mcp runs the agent, checks the gate afterwards, feeds a failure back while
/// turns remain, then moves on regardless -- and it is the highest-scoring arm on the
/// benchmark. The daemon instead refuses to complete until its gate passes, and on gated
/// tasks scored 0/4 at 42 min and $1.77 a run while satisfying that gate. This mode tests
/// whether the difference is the stopping rule rather than the execution model.
///
/// Carried on DaemonConfig rather than an env var: the first draft used one, and because
/// Rust runs tests in parallel threads of a single process it leaked into a sibling test.
#[test]
fn test_advisory_gate_advances_despite_a_failing_gate() {
    fn run(advisory: bool) -> bool {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .output()
            .unwrap();
        let state_path = repo.join(".agent-state/state.json");
        let state = init_state_milestones(
            Some("Goal"),
            vec![MilestoneSpec {
                title: "Never satisfiable".to_string(),
                verify_cmd: "false".to_string(),
                files: vec![],
                ..Default::default()
            }],
            None,
            true,
        )
        .unwrap();
        save_state_atomic(&state_path, &state).unwrap();
        let mut runner = DaemonRunner::new(DaemonConfig {
            state_path: state_path.clone(),
            driver_cmd: Some("true".to_string()),
            max_retries: 1,
            timeout_secs: 5,
            advisory_gate: advisory,
            advisory_review: false,
            review_blocking: false,
            auto_rollback: false,
            shunt_driver_output: false,
            guard_file_reads: false,
            guard_commands: false,
            impact_slicing: false,
        })
        .unwrap();
        for _ in 0..4 {
            match runner.run_step().unwrap() {
                DaemonStepOutcome::Advanced { .. } => return true,
                DaemonStepOutcome::Blocked { .. } => return false,
                DaemonStepOutcome::Failed { .. } => continue,
            }
        }
        false
    }

    assert!(
        !run(false),
        "strict mode must refuse to complete work that failed its gate"
    );
    assert!(
        run(true),
        "advisory mode must advance despite the gate still failing"
    );
}
