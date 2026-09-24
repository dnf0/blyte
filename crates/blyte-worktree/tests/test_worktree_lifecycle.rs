use blyte_worktree::manager::WorktreeManager;
use blyte_worktree::types::WorktreeConfig;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

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
    assert!(
        isolated
            .path
            .join(".venv")
            .join("bin")
            .join("python")
            .exists(),
        "cache should be copied"
    );

    // 2. List worktrees
    let list = manager.list().unwrap();
    assert!(
        list.iter()
            .any(|w| w.branch.as_deref() == Some("wt/milestone-1")),
        "list should contain wt/milestone-1"
    );

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
    isolated
        .merge_into_base("feat: squash milestone 1")
        .unwrap();
    assert!(
        repo_root.join("new_file.txt").exists(),
        "new_file.txt should exist in main repo after merge"
    );

    // 5. Destroy worktree
    let wt_path = isolated.path.clone();
    isolated.destroy(true).unwrap();
    assert!(!wt_path.exists(), "worktree directory should be removed");

    let list_after = manager.list().unwrap();
    assert!(
        !list_after
            .iter()
            .any(|w| w.branch.as_deref() == Some("wt/milestone-1")),
        "list should not contain wt/milestone-1"
    );
}

#[test]
fn test_ensure_task_worktree_and_lifecycle() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    let manager = WorktreeManager::new(repo_root).unwrap();

    // 1. Ensure task worktree creates .wt/task-1 and wt/task-1 branch
    let wt1 = manager.ensure_task_worktree("task-1", None).unwrap();
    assert!(
        wt1.path.exists(),
        "worktree directory .wt/task-1 should exist"
    );
    assert_eq!(wt1.path, repo_root.join(".wt").join("task-1"));
    assert_eq!(wt1.branch, "wt/task-1");

    // 2. Calling ensure_task_worktree idempotently returns existing worktree
    let wt1_again = manager.ensure_task_worktree("task-1", None).unwrap();
    assert_eq!(wt1_again.path, wt1.path);
    assert_eq!(wt1_again.branch, wt1.branch);

    // 3. Make a file and commit inside .wt/task-1
    fs::write(wt1.path.join("task1.txt"), b"task 1 content").unwrap();
    let add_status = Command::new("git")
        .args(["add", "task1.txt"])
        .current_dir(&wt1.path)
        .status()
        .unwrap();
    assert!(add_status.success());
    let commit_status = Command::new("git")
        .args(["commit", "-m", "feat: complete task 1"])
        .current_dir(&wt1.path)
        .status()
        .unwrap();
    assert!(commit_status.success());

    // Simulate an untracked conflicting file in repo_root (e.g. from container bind mount)
    fs::write(repo_root.join("task1.txt"), b"stale untracked content").unwrap();

    // 4. Merge and cleanup
    let wt1_path = wt1.path.clone();
    wt1.merge_and_cleanup("feat: complete task 1").unwrap();

    // Asserts commit is merged into main and .wt/task-1 is deleted
    assert!(
        repo_root.join("task1.txt").exists(),
        "task1.txt should exist in main repo after merge"
    );
    assert_eq!(
        fs::read(repo_root.join("task1.txt")).unwrap(),
        b"task 1 content",
        "task1.txt should have the authoritative committed worktree content"
    );
    assert!(
        !wt1_path.exists(),
        "worktree directory .wt/task-1 should be deleted"
    );

    // 5. Test abort: creates task-2, commits a change, calls abort()
    let wt2 = manager.ensure_task_worktree("task-2", None).unwrap();
    assert!(wt2.path.exists());
    assert_eq!(wt2.branch, "wt/task-2");

    fs::write(wt2.path.join("task2.txt"), b"task 2 content").unwrap();
    let add2_status = Command::new("git")
        .args(["add", "task2.txt"])
        .current_dir(&wt2.path)
        .status()
        .unwrap();
    assert!(add2_status.success());
    let commit2_status = Command::new("git")
        .args(["commit", "-m", "feat: incomplete task 2"])
        .current_dir(&wt2.path)
        .status()
        .unwrap();
    assert!(commit2_status.success());

    let wt2_path = wt2.path.clone();
    wt2.abort().unwrap();

    // Asserts not merged into main and worktree deleted
    assert!(
        !repo_root.join("task2.txt").exists(),
        "task2.txt should NOT be merged into main repo"
    );
    assert!(
        !wt2_path.exists(),
        "worktree directory .wt/task-2 should be deleted"
    );
}

#[test]
fn test_worktree_auto_commit_uncommitted_changes_on_merge() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    let manager = WorktreeManager::new(repo_root).unwrap();

    let wt = manager.ensure_task_worktree("task-auto", None).unwrap();
    let wt_path = wt.path.clone();

    // Write file directly into worktree WITHOUT running git add or git commit
    fs::write(wt.path.join("uncommitted.txt"), b"uncommitted content").unwrap();

    // Calling merge_and_cleanup should auto-commit and merge cleanly into main
    wt.merge_and_cleanup("feat: auto commit test").unwrap();

    assert!(
        repo_root.join("uncommitted.txt").exists(),
        "uncommitted.txt should be committed and merged into main repo"
    );
    assert_eq!(
        fs::read(repo_root.join("uncommitted.txt")).unwrap(),
        b"uncommitted content"
    );
    assert!(
        !wt_path.exists(),
        "worktree directory should be deleted after merge"
    );
}

#[test]
fn test_ensure_git_baseline_stages_preexisting_files() {
    use blyte_worktree::manager::ensure_git_baseline;

    let dir = TempDir::new().unwrap();
    let repo_root = dir.path();

    // Create preexisting files before git initialization
    fs::write(repo_root.join("main.db"), b"sqlite database content").unwrap();
    fs::write(repo_root.join("main.db-wal"), b"wal content").unwrap();

    // Call ensure_git_baseline
    ensure_git_baseline(repo_root);

    // Verify git repo initialized
    assert!(repo_root.join(".git").exists());

    // Create a worktree using WorktreeManager
    let manager = WorktreeManager::new(repo_root).unwrap();
    let wt = manager.ensure_task_worktree("task-1", None).unwrap();

    // Preexisting files MUST be present in the newly provisioned worktree!
    assert!(
        wt.path.join("main.db").exists(),
        "main.db must be checked out in worktree"
    );
    assert!(
        wt.path.join("main.db-wal").exists(),
        "main.db-wal must be checked out in worktree"
    );
    assert_eq!(
        fs::read(wt.path.join("main.db")).unwrap(),
        b"sqlite database content"
    );
    assert_eq!(
        fs::read(wt.path.join("main.db-wal")).unwrap(),
        b"wal content"
    );

    wt.abort().unwrap();
}
