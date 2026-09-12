//! Integration tests for default in-place state init and explicit --worktree flag.

use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_core::models::ExecutionState;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

fn init_git_repo(path: &Path) {
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .status()
        .unwrap();
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
fn test_state_init_default_in_place() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    let output = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "test",
            "--verify-cmd",
            "echo 1",
        ])
        .current_dir(repo_root)
        .output()
        .expect("Failed to execute state init");

    assert!(output.status.success(), "state init failed: {:?}", output);

    let state_file = repo_root.join(".agent-state").join("state.json");
    assert!(state_file.exists(), ".agent-state/state.json must exist");

    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();

    assert!(state.no_worktree, "state.no_worktree must be true by default");
    assert_eq!(state.active_worktree, None, "state.active_worktree must be None");
    assert!(!repo_root.join(".wt").exists(), ".wt/ directory must not exist");
}

#[test]
fn test_state_init_with_explicit_worktree() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    let output = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "test",
            "--verify-cmd",
            "echo 1",
            "--worktree",
        ])
        .current_dir(repo_root)
        .output()
        .expect("Failed to execute state init");

    assert!(output.status.success(), "state init failed: {:?}", output);

    let state_file = repo_root.join(".agent-state").join("state.json");
    assert!(state_file.exists(), ".agent-state/state.json must exist");

    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();

    assert!(!state.no_worktree, "state.no_worktree must be false when --worktree is passed");
    assert!(state.active_worktree.is_some(), "state.active_worktree must be Some");
    assert!(repo_root.join(".wt").join("task-1").exists(), ".wt/task-1 directory must exist");
}
