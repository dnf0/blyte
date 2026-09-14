use std::fs;
use std::process::Command;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{load_state, save_state_atomic};

/// Helper to configure a temporary git repository rooted on the main branch.
fn setup_temp_git_repo(path: &std::path::Path) {
    let init_res = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output();
    if let Ok(out) = init_res {
        if !out.status.success() {
            // Fallback for older git versions that do not support -b in init
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

    // Create an initial commit on main
    fs::write(path.join("README.md"), "# Test Repo\n").expect("write README");
    let _ = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(path)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(path)
        .output();
}

#[test]
fn test_daemon_automerge_on_completion() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    // Create an isolated worktree for feat/test-automerge
    let wt_dir = repo_root.join(".wt").join("feat-test-automerge");
    if let Some(parent) = wt_dir.parent() {
        fs::create_dir_all(parent).expect("create .wt dir");
    }

    let wt_add = Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "feat/test-automerge",
            &wt_dir.to_string_lossy(),
            "main",
        ])
        .current_dir(repo_root)
        .output()
        .expect("git worktree add");
    assert!(
        wt_add.status.success(),
        "Failed to create worktree: {}",
        String::from_utf8_lossy(&wt_add.stderr)
    );

    // Simulate driver actions by committing a file inside the isolated worktree
    fs::write(
        wt_dir.join("feature.txt"),
        "worktree feature implementation\n",
    )
    .expect("write feature.txt");
    let _ = Command::new("git")
        .args(["add", "feature.txt"])
        .current_dir(&wt_dir)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-m", "feat: implement test feature"])
        .current_dir(&wt_dir)
        .output();

    // Initialize execution state targeting the worktree
    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Implement feature".to_string(),
            verification_cmd: Some("test -f feature.txt".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: Some(wt_dir.clone()),
        active_branch: Some("feat/test-automerge".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: false,
        goal: Some("Verify daemon automerge lifecycle".to_string()),
        ..Default::default()
    };

    save_state_atomic(&state_path, &state).expect("save initial state");

    // Configure and run a single daemon step
    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 2,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).expect("create daemon runner");
    let outcome = runner.run_step().expect("daemon run_step failed");

    // 1. Assert step outcome transitions to completed (no next task)
    assert_eq!(
        outcome,
        DaemonStepOutcome::Advanced {
            completed_task_id: "task-1".to_string(),
            next_task_id: None,
        }
    );

    // 2. Assert state machine is Completed
    let final_state = load_state(&state_path).expect("load final state");
    assert_eq!(final_state.status, ExecutionStatus::Completed);
    assert_eq!(final_state.completed_tasks.len(), 1);

    // 3. Assert temporary worktree directory is cleaned up
    assert!(
        !wt_dir.exists(),
        "Worktree checkout directory should be removed after merge"
    );

    // 4. Assert worktree commits are merged into main
    let main_feature_file = repo_root.join("feature.txt");
    assert!(
        main_feature_file.exists(),
        "feature.txt must exist on main after automerge"
    );
    let content = fs::read_to_string(&main_feature_file).expect("read feature file");
    assert_eq!(content.trim(), "worktree feature implementation");

    // 5. Assert .agent-state/completion.json exists and contains structured metadata
    let completion_file = state_dir.join("completion.json");
    assert!(completion_file.exists(), "completion.json must exist");
    let completion_content = fs::read_to_string(&completion_file).expect("read completion.json");
    let completion_json: serde_json::Value =
        serde_json::from_str(&completion_content).expect("parse completion.json");

    assert_eq!(completion_json["status"], "completed");
    assert_eq!(completion_json["slug"], "default");
    assert_eq!(completion_json["base_branch"], "main");
    assert_eq!(completion_json["milestones_completed"], 1);
    assert_eq!(completion_json["worktree_merged"], true);
    assert!(completion_json["completed_at"].is_string());

    // 6. Assert scoped completions/<slug>.json also exists
    let scoped_completion_file = state_dir.join("completions").join("default.json");
    assert!(
        scoped_completion_file.exists(),
        "completions/default.json must exist"
    );
}

#[test]
fn test_daemon_completion_in_place_mode() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    // In-place execution directly in repo root
    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "In-place task".to_string(),
            verification_cmd: Some("test -f README.md".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: None,
        base_branch: Some("main".to_string()),
        no_worktree: true,
        ..Default::default()
    };

    save_state_atomic(&state_path, &state).expect("save initial state");

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 2,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).expect("create daemon runner");
    let outcome = runner.run_step().expect("daemon run_step failed");

    assert_eq!(
        outcome,
        DaemonStepOutcome::Advanced {
            completed_task_id: "task-1".to_string(),
            next_task_id: None,
        }
    );

    let completion_file = state_dir.join("completion.json");
    assert!(completion_file.exists(), "completion.json must exist");
    let completion_content = fs::read_to_string(&completion_file).expect("read completion.json");
    let completion_json: serde_json::Value =
        serde_json::from_str(&completion_content).expect("parse completion.json");

    assert_eq!(completion_json["status"], "completed");
    assert_eq!(completion_json["worktree_merged"], false);
}

#[test]
fn test_daemon_run_to_completion_avoids_duplicate_finalization() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    setup_temp_git_repo(repo_root);

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Autonomous task".to_string(),
            verification_cmd: Some("test -f README.md".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: None,
        base_branch: Some("main".to_string()),
        no_worktree: true,
        ..Default::default()
    };

    save_state_atomic(&state_path, &state).expect("save initial state");

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 2,
        timeout_secs: 10,
    };

    let mut runner = DaemonRunner::new(config).expect("create daemon runner");
    let report = runner
        .run_to_completion()
        .expect("run_to_completion must succeed");

    assert_eq!(report.status, ExecutionStatus::Completed);
    assert_eq!(report.milestones_completed, 1);
    assert!(report.completion_file.exists());

    // Calling run_to_completion again when already completed reuses existing artifact
    let mut runner2 = DaemonRunner::new(DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 2,
        timeout_secs: 10,
    })
    .expect("create second runner");

    let report2 = runner2
        .run_to_completion()
        .expect("second run_to_completion should return existing report");
    assert_eq!(report2.status, ExecutionStatus::Completed);
    assert_eq!(report2.completed_at, report.completed_at);
}
