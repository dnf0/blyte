use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{load_state, save_state_atomic};

/// Helper to configure a temporary git repository rooted on the main branch.
fn setup_temp_git_repo(path: &Path) -> String {
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

    // Create .gitignore to prevent pycache from polluting git status/diff
    fs::write(path.join(".gitignore"), "__pycache__/\n*.pyc\n").expect("write .gitignore");

    // Create an initial tracked file and commit
    fs::write(path.join("tracked.txt"), "initial content\n").expect("write tracked.txt");
    let _ = Command::new("git")
        .args(["add", "."])
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
fn test_daemon_impact_slicing_fast_fail_skips_full_gate() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(!initial_commit.is_empty());

    // Create source and failing test files
    fs::create_dir_all(repo_root.join("src")).expect("create src dir");
    fs::create_dir_all(repo_root.join("tests")).expect("create tests dir");
    fs::write(
        repo_root.join("src/lib.py"),
        "def compute(a, b): return a + b\n",
    )
    .expect("write src/lib.py");
    fs::write(
        repo_root.join("tests/test_lib.py"),
        "def test_failing_slice(): assert False\n",
    )
    .expect("write tests/test_lib.py");

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .output();

    // Initialize execution state targeting in-place repository
    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-impact-fail-1".to_string(),
            title: "Task with failing impacted test slice".to_string(),
            verification_cmd: Some(
                "pytest tests/ && touch full_gate_ran.txt && exit 0".to_string(),
            ),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test fast-fail test impact slicing".to_string()),
        ..Default::default()
    };
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
        impact_slicing: true,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.step().expect("runner step");

    // 1. Outcome must be Failed
    assert!(
        matches!(outcome, DaemonStepOutcome::Failed { .. }),
        "expected Failed outcome, got {:?}",
        outcome
    );

    // 2. Full gate was skipped: full_gate_ran.txt must NOT exist
    assert!(
        !repo_root.join("full_gate_ran.txt").exists(),
        "full gate command should have been skipped on fast fail"
    );

    // 3. State notes must contain [impact-fail]
    let updated_state = load_state(&state_path).expect("load state");
    let has_impact_fail_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[impact-fail]"));
    assert!(
        has_impact_fail_note,
        "state notes should contain [impact-fail], got: {:?}",
        updated_state.notes
    );
}

#[test]
fn test_daemon_impact_slicing_passing_slice_executes_full_gate() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(!initial_commit.is_empty());

    // Create source and passing test files
    fs::create_dir_all(repo_root.join("src")).expect("create src dir");
    fs::create_dir_all(repo_root.join("tests")).expect("create tests dir");
    fs::write(
        repo_root.join("src/lib.py"),
        "def compute(a, b): return a + b\n",
    )
    .expect("write src/lib.py");
    fs::write(
        repo_root.join("tests/test_lib.py"),
        "def test_passing_slice(): assert True\n",
    )
    .expect("write tests/test_lib.py");

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .output();

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-impact-pass-1".to_string(),
            title: "Task with passing impacted test slice".to_string(),
            verification_cmd: Some(
                "pytest tests/ && touch full_gate_ran.txt && exit 0".to_string(),
            ),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test passing slice executes full gate".to_string()),
        ..Default::default()
    };
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
        impact_slicing: true,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.step().expect("runner step");

    // 1. Outcome must be Advanced
    assert!(
        outcome.is_advanced(),
        "expected Advanced outcome, got {:?}",
        outcome
    );

    // 2. Full gate was executed: full_gate_ran.txt MUST exist
    assert!(
        repo_root.join("full_gate_ran.txt").exists(),
        "full gate command should have executed and touched full_gate_ran.txt"
    );

    // 3. State notes must contain [impact-pass]
    let updated_state = load_state(&state_path).expect("load state");
    let has_impact_pass_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[impact-pass]"));
    assert!(
        has_impact_pass_note,
        "state notes should contain [impact-pass], got: {:?}",
        updated_state.notes
    );
}

#[test]
fn test_daemon_impact_slicing_zero_impacted_falls_back_to_full_gate() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(!initial_commit.is_empty());

    // Modify a non-source or non-test file (no test candidates exist)
    fs::write(repo_root.join("docs.txt"), "documentation update\n").expect("write docs.txt");
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .output();

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-zero-impacted-1".to_string(),
            title: "Task with zero impacted test files".to_string(),
            verification_cmd: Some("touch full_gate_ran.txt && exit 0".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test fallback when zero tests are impacted".to_string()),
        ..Default::default()
    };
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
        impact_slicing: true,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.step().expect("runner step");

    // 1. Outcome must be Advanced
    assert!(
        outcome.is_advanced(),
        "expected Advanced outcome, got {:?}",
        outcome
    );

    // 2. Full gate was executed: full_gate_ran.txt MUST exist
    assert!(
        repo_root.join("full_gate_ran.txt").exists(),
        "full gate command should have executed and touched full_gate_ran.txt"
    );

    // 3. Neither impact-fail nor impact-pass should be recorded
    let updated_state = load_state(&state_path).expect("load state");
    let has_impact_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[impact-pass]") || n.note.contains("[impact-fail]"));
    assert!(
        !has_impact_note,
        "state notes should not contain impact notes when no tests impacted, got: {:?}",
        updated_state.notes
    );
}

#[test]
fn test_daemon_impact_slicing_disabled_by_default() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(!initial_commit.is_empty());

    // Create failing test file that would fail if sliced
    fs::create_dir_all(repo_root.join("src")).expect("create src dir");
    fs::create_dir_all(repo_root.join("tests")).expect("create tests dir");
    fs::write(
        repo_root.join("src/lib.py"),
        "def compute(a, b): return a + b\n",
    )
    .expect("write src/lib.py");
    fs::write(
        repo_root.join("tests/test_lib.py"),
        "def test_failing_slice(): assert False\n",
    )
    .expect("write tests/test_lib.py");

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .output();

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-disabled-impact-1".to_string(),
            title: "Task with impact slicing disabled".to_string(),
            verification_cmd: Some("touch full_gate_ran.txt && exit 0".to_string()),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test impact slicing disabled by default".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).expect("save state");

    // impact_slicing is false
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
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.step().expect("runner step");

    assert!(outcome.is_advanced());
    assert!(repo_root.join("full_gate_ran.txt").exists());

    let updated_state = load_state(&state_path).expect("load state");
    let has_impact_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[impact-pass]") || n.note.contains("[impact-fail]"));
    assert!(!has_impact_note);
}

#[test]
fn test_daemon_impact_slicing_fast_fail_with_auto_rollback() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let repo_root = temp_dir.path();
    let initial_commit = setup_temp_git_repo(repo_root);
    assert!(!initial_commit.is_empty());

    // Create source and failing test files
    fs::create_dir_all(repo_root.join("src")).expect("create src dir");
    fs::create_dir_all(repo_root.join("tests")).expect("create tests dir");
    fs::write(
        repo_root.join("src/lib.py"),
        "def compute(a, b): return a + b\n",
    )
    .expect("write src/lib.py");
    fs::write(
        repo_root.join("tests/test_lib.py"),
        "def test_failing_slice(): assert False\n",
    )
    .expect("write tests/test_lib.py");

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .output();

    let state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let state_path = state_dir.join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-rollback-1".to_string(),
            title: "Task with failing slice and auto-rollback".to_string(),
            verification_cmd: Some(
                "pytest tests/ && touch full_gate_ran.txt && exit 0".to_string(),
            ),
            ..Default::default()
        }),
        pending_tasks: Vec::new(),
        active_worktree: None,
        active_branch: Some("main".to_string()),
        base_branch: Some("main".to_string()),
        no_worktree: true,
        goal: Some("Test fast-fail with auto-rollback".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).expect("save state");

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: None,
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
        shunt_driver_output: false,
        impact_slicing: true,
    };

    let mut runner = DaemonRunner::new(config).expect("create runner");
    let outcome = runner.step().expect("runner step");

    assert!(matches!(outcome, DaemonStepOutcome::Failed { .. }));
    assert!(!repo_root.join("full_gate_ran.txt").exists());

    // Both [impact-fail] and [rollback] notes should be in state notes
    let updated_state = load_state(&state_path).expect("load state");
    assert!(updated_state.notes.iter().any(|n| n.note.contains("[impact-fail]")));
    assert!(updated_state.notes.iter().any(|n| n.note.contains("[rollback]")));
}

