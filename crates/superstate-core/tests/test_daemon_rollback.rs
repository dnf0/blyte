use std::fs;
use std::process::Command;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome, RollbackPolicy};
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
fn test_daemon_preserves_work_when_a_failing_attempt_changed_the_tree() {
    // A failing attempt that moved the working tree is partial progress. Wiping it forces the
    // worker to rebuild from scratch every turn, which is what EXP-071 measured: 30 verification
    // failures produced 30 rollbacks, and one worker's committed feature was reset five times
    // before being rebuilt identically. See FINDINGS.md 13.2.
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(
        !initial_commit.is_empty(),
        "initial commit must not be empty"
    );

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-progress-1".to_string(),
            title: "Failing attempt that still moved the tree".to_string(),
            verification_cmd: Some("test -f milestone_done.txt".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test that progress survives a failed gate".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).expect("save state");

    // The worker does real work; the gate still fails because milestone_done.txt is absent.
    let driver_script = "echo 'partial implementation' > tracked.txt && \
                         echo 'new source file' > feature.rs";

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script.to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
        rollback_policy: RollbackPolicy::OnStall,
        shunt_driver_output: false,
        guard_file_reads: false,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.run_step().expect("run_step");

    assert!(
        matches!(outcome, DaemonStepOutcome::Failed { .. }),
        "expected Failed outcome, got {:?}",
        outcome
    );

    // The partial work must survive, even though auto_rollback is enabled.
    let tracked_content =
        fs::read_to_string(repo_root.join("tracked.txt")).expect("read tracked.txt");
    assert_eq!(
        tracked_content.trim(),
        "partial implementation",
        "a failing attempt that changed the tree must keep its work"
    );
    assert!(
        repo_root.join("feature.rs").exists(),
        "newly created files must survive a failed gate when the attempt made progress"
    );

    let updated_state = load_state(&state_path).expect("load updated state");
    assert!(
        updated_state
            .notes
            .iter()
            .all(|n| !n.note.contains("[rollback]")),
        "no [rollback] note may be recorded for an attempt that moved the tree"
    );
}

#[test]
fn test_daemon_rolls_back_when_an_attempt_changed_nothing() {
    // The escape valve: a worker that changed nothing and still failed is stuck, so returning it
    // to the milestone baseline is the one case where discarding the tree helps.
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-stalled-1".to_string(),
            title: "Worker that stops changing the tree".to_string(),
            verification_cmd: Some("test -f milestone_done.txt".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test the stalled-worker escape valve".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).expect("save state");

    // Attempt 1 writes files (progress, no rollback). Attempt 2 touches nothing at all, so the
    // workspace fingerprint is unchanged and the attempt counts as a stall.
    //
    // NOTE: `workspace_fingerprint` hashes size and mtime, not content, so a worker that rewrites
    // identical bytes still reads as progress. The driver must therefore genuinely no-op here.
    let driver_script = "if [ ! -f marker.txt ]; then \
                           echo 'corrupted driver content' > tracked.txt; \
                           echo 'untracked artifact' > untracked.txt; \
                           mkdir -p untracked_dir; \
                           echo 'nested' > untracked_dir/nested.txt; \
                           touch marker.txt; \
                         fi";

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script.to_string()),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
        rollback_policy: RollbackPolicy::OnStall,
        shunt_driver_output: false,
        guard_file_reads: false,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");

    let first = runner.run_step().expect("run_step 1");
    assert!(matches!(first, DaemonStepOutcome::Failed { .. }));
    assert!(
        repo_root.join("untracked.txt").exists(),
        "attempt 1 made progress, so its work must survive"
    );

    let second = runner.run_step().expect("run_step 2");
    assert!(matches!(second, DaemonStepOutcome::Failed { .. }));

    // Attempt 2 changed nothing, so the tree returns to the milestone baseline.
    let tracked_content =
        fs::read_to_string(repo_root.join("tracked.txt")).expect("read tracked.txt");
    assert_eq!(
        tracked_content, "initial content\n",
        "a stalled attempt must restore the tracked file to the baseline commit"
    );
    assert!(
        !repo_root.join("untracked.txt").exists(),
        "untracked.txt should be removed by git clean -fd on a stalled attempt"
    );
    assert!(
        !repo_root.join("untracked_dir").exists(),
        "untracked_dir should be removed by git clean -fd on a stalled attempt"
    );

    let updated_state = load_state(&state_path).expect("load updated state");
    let rollback_note = updated_state
        .notes
        .iter()
        .find(|n| n.note.contains("[rollback]"))
        .expect("state.notes must contain a [rollback] note for the stalled attempt");
    assert!(
        rollback_note
            .note
            .contains(&format!("baseline commit {}", initial_commit)),
        "note should reference baseline commit SHA: {}",
        rollback_note.note
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
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: false,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: false,
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

#[test]
fn the_default_policy_resets_even_an_attempt_that_made_progress() {
    // Manifesto Invariant III: "If an execution turn fails its verification gate, the control
    // plane executes a scoped, deterministic hard rollback before the agent is allowed to
    // retry ... not a corrupted workspace." That is the default, and it must stay the default
    // unless a run deliberately selects otherwise.
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");
    save_state_atomic(
        &state_path,
        &ExecutionState {
            status: ExecutionStatus::InProgress,
            step: 1,
            active_task: Some(TaskSpec {
                id: "task-default-1".to_string(),
                title: "Default policy".to_string(),
                verification_cmd: Some("test -f milestone_done.txt".to_string()),
                ..Default::default()
            }),
            no_worktree: true,
            active_branch: Some("main".to_string()),
            base_branch: Some("main".to_string()),
            ..Default::default()
        },
    )
    .expect("save state");

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some("echo 'real work' > tracked.txt && echo new > feature.rs".to_string()),
        max_retries: 3,
        timeout_secs: 10,
        auto_rollback: true,
        // deliberately not overridden: this asserts what a run gets by default
        ..Default::default()
    };
    assert_eq!(
        config.rollback_policy,
        RollbackPolicy::Always,
        "the default policy must be the one the manifesto mandates"
    );

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.run_step().expect("run_step");
    assert!(matches!(outcome, DaemonStepOutcome::Failed { .. }));

    assert_eq!(
        fs::read_to_string(repo_root.join("tracked.txt")).expect("read"),
        "initial content\n",
        "under the default policy a failed attempt is reset even though it changed the tree"
    );
    assert!(!repo_root.join("feature.rs").exists());
}

#[test]
fn on_regression_keeps_incomplete_work_but_resets_a_broken_suite() {
    // A failing gate cannot separate "broken" from "incomplete"; a `&&` chain can. The first
    // clause is the reproduction test: if it still passes while the chain failed, a later
    // clause broke, which is a regression. If it fails, the work is merely unfinished.
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");

    for (label, driver, expect_reset) in [
        // The driver writes `repro` but not `suite`: first clause passes, second fails -> the
        // suite regressed, so the tree is reset.
        ("regression", "echo work > tracked.txt && touch repro", true),
        // Neither marker: the first clause fails, so the work is incomplete and is kept.
        ("incomplete", "echo work > tracked.txt", false),
    ] {
        let state_path = state_dir.join(format!("{label}.json"));
        save_state_atomic(
            &state_path,
            &ExecutionState {
                status: ExecutionStatus::InProgress,
                step: 1,
                active_task: Some(TaskSpec {
                    id: format!("task-{label}"),
                    title: "Regression policy".to_string(),
                    verification_cmd: Some("test -f repro && test -f suite".to_string()),
                    ..Default::default()
                }),
                no_worktree: true,
                active_branch: Some("main".to_string()),
                base_branch: Some("main".to_string()),
                ..Default::default()
            },
        )
        .expect("save state");

        let mut runner = DaemonRunner::new(DaemonConfig {
            state_path: state_path.clone(),
            driver_cmd: Some(driver.to_string()),
            max_retries: 3,
            timeout_secs: 10,
            auto_rollback: true,
            rollback_policy: RollbackPolicy::OnRegression,
            ..Default::default()
        })
        .expect("create runner");
        let outcome = runner.run_step().expect("run_step");
        assert!(
            matches!(outcome, DaemonStepOutcome::Failed { .. }),
            "{label}"
        );

        let tracked = fs::read_to_string(repo_root.join("tracked.txt")).expect("read");
        if expect_reset {
            assert_eq!(
                tracked, "initial content\n",
                "{label}: a regression must reset"
            );
        } else {
            assert_eq!(
                tracked.trim(),
                "work",
                "{label}: incomplete work must survive"
            );
        }
        // Reset the fixture for the next case.
        let _ = std::process::Command::new("git")
            .args(["reset", "--hard", "HEAD"])
            .current_dir(repo_root)
            .output();
        let _ = std::process::Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(repo_root)
            .output();
    }
}
