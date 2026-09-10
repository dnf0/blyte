use std::fs;
use std::process::Command;
use tempfile::TempDir;
use superstate_worktree::manager::WorktreeManager;
use superstate_worktree::types::WorktreeConfig;

fn init_test_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    // git init -b main
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .status()
        .unwrap();
    assert!(status.success());

    // git config user.name / email
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

    // Add baseline commit
    fs::write(path.join("README.md"), b"# Test Project\n").unwrap();
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

    dir
}

#[test]
fn test_worktree_lifecycle_full() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    // Create a mock cache directory in the repo
    let venv_dir = repo_root.join(".venv").join("bin");
    fs::create_dir_all(&venv_dir).unwrap();
    fs::write(venv_dir.join("python"), b"#!/bin/sh\necho 3.12\n").unwrap();

    let manager = WorktreeManager::new(repo_root).unwrap();

    let config = WorktreeConfig {
        base_path: repo_root.to_path_buf(),
        branch: "wt/milestone-1".to_string(),
        base_branch: Some("main".to_string()),
        worktree_path: None,
        cache_dirs: vec![".venv".to_string()],
    };

    // 1. Create worktree
    let isolated = manager.create(&config).unwrap();
    assert!(isolated.path.exists(), "worktree path should exist");
    assert!(isolated.path.join(".venv").join("bin").join("python").exists(), "cache should be copied");

    // 2. List worktrees
    let list = manager.list().unwrap();
    assert!(list.iter().any(|w| w.branch.as_deref() == Some("wt/milestone-1")), "list should contain wt/milestone-1");

    // 3. Make changes in worktree
    fs::write(isolated.path.join("new_file.txt"), b"milestone completed").unwrap();
    let add_status = Command::new("git")
        .args(["add", "new_file.txt"])
        .current_dir(&isolated.path)
        .status()
        .unwrap();
    assert!(add_status.success());
    let commit_status = Command::new("git")
        .args(["commit", "-m", "feat: milestone 1 completed"])
        .current_dir(&isolated.path)
        .status()
        .unwrap();
    assert!(commit_status.success());

    // 4. Merge into base
    isolated.merge_into_base("feat: squash milestone 1").unwrap();
    assert!(repo_root.join("new_file.txt").exists(), "new_file.txt should exist in main repo after merge");

    // 5. Destroy worktree
    let wt_path = isolated.path.clone();
    isolated.destroy(true).unwrap();
    assert!(!wt_path.exists(), "worktree directory should be removed");

    let list_after = manager.list().unwrap();
    assert!(!list_after.iter().any(|w| w.branch.as_deref() == Some("wt/milestone-1")), "list should not contain wt/milestone-1");
}
