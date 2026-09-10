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

#[test]
fn test_cli_cannot_remove_or_merge_primary_worktree() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Attempting to remove main worktree must fail
    let remove_main = Command::new(bin())
        .args(["worktree", "remove", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!remove_main.status.success());
    let err_str = String::from_utf8_lossy(&remove_main.stderr);
    assert!(err_str.contains("Cannot remove primary worktree"), "Expected guard against removing primary worktree, got: {}", err_str);

    // Attempting to merge main into itself must fail
    let merge_main = Command::new(bin())
        .args(["worktree", "merge", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merge_main.status.success());
    let err_str = String::from_utf8_lossy(&merge_main.stderr);
    assert!(err_str.contains("Cannot merge primary worktree"), "Expected guard against merging primary worktree, got: {}", err_str);
}

#[test]
fn test_cli_no_cache_flag_prevents_cache_copy() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Create a mock .venv cache
    let venv = dir.path().join(".venv");
    fs::create_dir_all(&venv).unwrap();
    fs::write(venv.join("marker.txt"), b"cache").unwrap();

    // Create worktree with --no-cache
    let create_out = Command::new(bin())
        .args(["worktree", "create", "wt/no-cache-branch", "--no-cache"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(create_out.status.success(), "create failed: {:?}", create_out);

    let wt_path = dir.path().join(".wt").join("wt-no-cache-branch");
    assert!(wt_path.exists());
    assert!(!wt_path.join(".venv").exists(), ".venv should not have been copied due to --no-cache");
}

#[test]
fn test_cli_operations_from_inside_worktree() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // 1. Create worktree
    let create_out = Command::new(bin())
        .args(["worktree", "create", "wt/inner-branch"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(create_out.status.success());

    let wt_path = dir.path().join(".wt").join("wt-inner-branch");
    assert!(wt_path.exists());

    // 2. Commit inside worktree
    fs::write(wt_path.join("inner.txt"), b"inner content").unwrap();
    Command::new("git").args(["add", "inner.txt"]).current_dir(&wt_path).status().unwrap();
    Command::new("git").args(["commit", "-m", "feat: inner"]).current_dir(&wt_path).status().unwrap();

    // 3. Merge *from inside* the worktree directory
    let merge_out = Command::new(bin())
        .args(["worktree", "merge", "wt/inner-branch"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(merge_out.status.success(), "merge from inside worktree failed: {:?}", merge_out);
    assert!(dir.path().join("inner.txt").exists(), "inner.txt should exist in main repo root");

    // 4. Remove *from main dir* (since removing the worktree while CWD is inside it is an OS lock issue)
    let remove_out = Command::new(bin())
        .args(["worktree", "remove", "wt/inner-branch"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(remove_out.status.success(), "remove failed: {:?}", remove_out);
    assert!(!wt_path.exists());
}

#[test]
fn test_cli_merge_base_mismatch_error() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Create worktree
    let create_out = Command::new(bin())
        .args(["worktree", "create", "wt/mismatch-branch"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(create_out.status.success());

    // Try to merge with non-matching --base
    let merge_out = Command::new(bin())
        .args(["worktree", "merge", "wt/mismatch-branch", "--base", "release-2.0"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merge_out.status.success());
    let err_str = String::from_utf8_lossy(&merge_out.stderr);
    assert!(err_str.contains("expected target base branch 'release-2.0'"), "Expected mismatch error, got: {}", err_str);
}

#[test]
fn test_cli_multiple_caches_copied() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Setup multiple mock cache dirs
    let c1 = dir.path().join("cache1");
    let c2 = dir.path().join("cache2");
    fs::create_dir_all(&c1).unwrap();
    fs::create_dir_all(&c2).unwrap();
    fs::write(c1.join("c1.txt"), b"c1").unwrap();
    fs::write(c2.join("c2.txt"), b"c2").unwrap();

    let create_out = Command::new(bin())
        .args(["worktree", "create", "wt/multi-cache-branch", "--caches", "cache1,cache2"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(create_out.status.success(), "create failed: {:?}", create_out);

    let wt_path = dir.path().join(".wt").join("wt-multi-cache-branch");
    assert!(wt_path.join("cache1").join("c1.txt").exists());
    assert!(wt_path.join("cache2").join("c2.txt").exists());
}
