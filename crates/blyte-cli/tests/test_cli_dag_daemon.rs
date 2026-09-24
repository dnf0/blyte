//! Integration tests for arbitrary DAG milestone execution via CLI and daemon.

use blyte_core::models::{ExecutionState, ExecutionStatus};
use std::fs;
use std::path::Path;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

fn init_git_repo(path: &Path, branch: &str) {
    let status = Command::new("git")
        .args(["init", "-b", branch])
        .current_dir(path)
        .status()
        .expect("git init failed");
    assert!(status.success());

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .status()
        .unwrap();

    fs::write(path.join("README.md"), b"# Base Repository\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "chore: initial baseline"])
        .current_dir(path)
        .status()
        .unwrap();
}

#[test]
fn test_cli_diamond_dag_daemon_execution() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "main");

    // Diamond DAG:
    //      task-a
    //      /    \
    //   task-b  task-c
    //      \    /
    //      task-d
    let diamond_json = r#"[
        {"id": "task-a", "title": "Task A", "verify_cmd": "test -f a.txt", "depends_on": []},
        {"id": "task-b", "title": "Task B", "verify_cmd": "test -f b.txt", "depends_on": ["task-a"]},
        {"id": "task-c", "title": "Task C", "verify_cmd": "test -f c.txt", "depends_on": ["task-a"]},
        {"id": "task-d", "title": "Task D", "verify_cmd": "test -f d.txt", "depends_on": ["task-b", "task-c"]}
    ]"#;

    let init_output = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            diamond_json,
            "--no-worktree",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run blyte state init");

    assert!(
        init_output.status.success(),
        "state init failed: stderr={}",
        String::from_utf8_lossy(&init_output.stderr)
    );

    let state_file = repo_root.join(".agent-state").join("state.json");
    assert!(state_file.exists(), "state file must exist after init");

    // Mock driver script touches the file corresponding to active task
    let driver_script = r#"
case "$BLYTE_ACTIVE_TASK" in
    task-a) echo "done a" > a.txt ;;
    task-b) echo "done b" > b.txt ;;
    task-c) echo "done c" > c.txt ;;
    task-d) echo "done d" > d.txt ;;
esac
"#;

    let daemon_output = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--driver",
            driver_script,
            "--max-retries",
            "2",
            "--timeout",
            "10",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run blyte state daemon");

    assert!(
        daemon_output.status.success(),
        "daemon run failed: stderr={}, stdout={}",
        String::from_utf8_lossy(&daemon_output.stderr),
        String::from_utf8_lossy(&daemon_output.stdout)
    );

    // Verify all files were created
    assert!(repo_root.join("a.txt").exists(), "a.txt must exist");
    assert!(repo_root.join("b.txt").exists(), "b.txt must exist");
    assert!(repo_root.join("c.txt").exists(), "c.txt must exist");
    assert!(repo_root.join("d.txt").exists(), "d.txt must exist");

    // Verify state reaches completed with all 4 tasks in completed_tasks
    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();
    assert_eq!(state.status, ExecutionStatus::Completed);
    assert!(state.active_task.is_none());
    assert_eq!(state.completed_tasks.len(), 4);

    let completed_ids: Vec<&str> = state
        .completed_tasks
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(
        completed_ids,
        vec!["task-a", "task-b", "task-c", "task-d"],
        "all 4 tasks must be in completed_tasks in execution order"
    );

    // Verify git history has milestone checkpoint commits for all 4 tasks
    let log_output = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(&repo_root)
        .output()
        .expect("git log failed");
    assert!(log_output.status.success());
    let log_text = String::from_utf8_lossy(&log_output.stdout);

    assert!(
        log_text.contains("checkpoint completed milestone task-a: Task A"),
        "git log must contain checkpoint for task-a, got:\n{}",
        log_text
    );
    assert!(
        log_text.contains("checkpoint completed milestone task-b: Task B"),
        "git log must contain checkpoint for task-b, got:\n{}",
        log_text
    );
    assert!(
        log_text.contains("checkpoint completed milestone task-c: Task C"),
        "git log must contain checkpoint for task-c, got:\n{}",
        log_text
    );
    assert!(
        log_text.contains("checkpoint completed milestone task-d: Task D"),
        "git log must contain checkpoint for task-d, got:\n{}",
        log_text
    );
}

#[test]
fn test_cli_deadlocked_dag_daemon_halts_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "main");

    // Deadlocked DAG: task-1 succeeds immediately, but task-2 depends on a nonexistent task
    let deadlocked_json = r#"[
        {"id": "task-1", "title": "Task 1", "verify_cmd": "true", "depends_on": []},
        {"id": "task-2", "title": "Task 2", "verify_cmd": "true", "depends_on": ["nonexistent-task"]}
    ]"#;

    let init_output = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            deadlocked_json,
            "--no-worktree",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run blyte state init");

    assert!(
        init_output.status.success(),
        "state init failed: stderr={}",
        String::from_utf8_lossy(&init_output.stderr)
    );

    let state_file = repo_root.join(".agent-state").join("state.json");
    assert!(state_file.exists());

    // Run daemon: task-1 passes, but advancement cannot satisfy task-2's dependency
    let daemon_output = Command::new(bin())
        .args(["state", "daemon", "--max-retries", "1", "--timeout", "5"])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run blyte state daemon");

    // Daemon should halt (exiting non-zero due to blocked condition)
    assert!(
        !daemon_output.status.success(),
        "daemon should exit non-zero when blocked, stdout={}, stderr={}",
        String::from_utf8_lossy(&daemon_output.stdout),
        String::from_utf8_lossy(&daemon_output.stderr)
    );

    // Verify state file has status: "blocked"
    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();
    assert_eq!(
        state.status,
        ExecutionStatus::Blocked,
        "state status must be Blocked"
    );

    // Verify state notes contain a [dag-blocked] note
    let dag_blocked_note = state
        .notes
        .iter()
        .find(|n| n.note.contains("[dag-blocked]"));
    assert!(
        dag_blocked_note.is_some(),
        "state notes must contain [dag-blocked] note, notes={:?}",
        state.notes
    );

    let note_text = &dag_blocked_note.unwrap().note;
    assert!(
        note_text.contains("Milestone task-1 completed, but remaining tasks cannot advance"),
        "note text must describe the blocked milestone, got: {}",
        note_text
    );
}

#[test]
fn test_cli_init_initial_deadlock_error_reporting() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "main");

    // Cyclic dependency with no starting task
    let cyclic_json = r#"[
        {"id": "task-1", "title": "Task 1", "verify_cmd": "true", "depends_on": ["task-2"]},
        {"id": "task-2", "title": "Task 2", "verify_cmd": "true", "depends_on": ["task-1"]}
    ]"#;

    let init_output = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            cyclic_json,
            "--no-worktree",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run blyte state init");

    assert!(
        !init_output.status.success(),
        "state init should fail on cyclical dependency without starting task"
    );

    let stderr = String::from_utf8_lossy(&init_output.stderr);
    assert!(
        stderr.contains("Failed to initialize state from milestones")
            && stderr.contains("satisfied dependencies"),
        "stderr should cleanly report failure to find starting task with satisfied dependencies: {}",
        stderr
    );
}
