//! End-to-end integration tests for `superstate worktree` CLI commands.

use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

fn init_git_repo(path: &std::path::Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .status()
        .unwrap();

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

    fs::write(path.join("README.md"), b"# Base\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(path)
        .status()
        .unwrap();
}

#[test]
fn test_cli_worktree_lifecycle() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // 1. Create worktree via CLI
    let create_out = Command::new(bin())
        .args(["worktree", "create", "wt/cli-branch", "--base", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(create_out.status.success(), "create failed: {:?}", create_out);

    // 2. List worktrees via CLI
    let list_out = Command::new(bin())
        .args(["worktree", "list"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(list_out.status.success(), "list failed: {:?}", list_out);
    let list_str = String::from_utf8_lossy(&list_out.stdout);
    assert!(list_str.contains("wt/cli-branch"), "list should contain wt/cli-branch, got: {}", list_str);

    // 3. Make commit inside worktree
    let wt_path = dir.path().join(".wt").join("wt-cli-branch");
    assert!(wt_path.exists(), "worktree path does not exist");
    fs::write(wt_path.join("file.txt"), b"created in worktree").unwrap();

    Command::new("git")
        .args(["add", "file.txt"])
        .current_dir(&wt_path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "feat: from worktree"])
        .current_dir(&wt_path)
        .status()
        .unwrap();

    // 4. Merge via CLI
    let merge_out = Command::new(bin())
        .args(["worktree", "merge", "wt/cli-branch", "--message", "feat: merge worktree"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(merge_out.status.success(), "merge failed: {:?}", merge_out);
    assert!(dir.path().join("file.txt").exists(), "file.txt should exist in main repo after merge");

    // 5. Remove worktree via CLI
    let remove_out = Command::new(bin())
        .args(["worktree", "remove", "wt/cli-branch", "--force"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(remove_out.status.success(), "remove failed: {:?}", remove_out);
    assert!(!wt_path.exists(), "worktree dir should be removed");

    // 6. Verify worktree list no longer contains branch
    let list_after_out = Command::new(bin())
        .args(["worktree", "list"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(list_after_out.status.success());
    let list_after_str = String::from_utf8_lossy(&list_after_out.stdout);
    assert!(!list_after_str.contains("wt/cli-branch"), "list should no longer contain wt/cli-branch");
}
