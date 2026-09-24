use blyte_worktree::manager::WorktreeManager;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn init_test_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

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
fn test_current_branch_detection() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    let status = Command::new("git")
        .args(["checkout", "-b", "feat/test-feature"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(status.success());

    let manager = WorktreeManager::new(repo_root).unwrap();
    assert_eq!(
        manager.current_branch().unwrap(),
        "feat/test-feature",
        "current_branch should return the active feature branch"
    );
}

#[test]
fn test_milestone_merges_to_feature_branch() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    // Create and checkout feature branch
    let status = Command::new("git")
        .args(["checkout", "-b", "feat/payment-integration"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(status.success());

    let manager = WorktreeManager::new(repo_root).unwrap();

    // Provision worktree for task-1 with base_branch: None (should target feat/payment-integration)
    let wt = manager.ensure_task_worktree("task-1", None).unwrap();
    assert_eq!(
        wt.base_branch, "feat/payment-integration",
        "base_branch should default to current feature branch"
    );

    // Create a file and commit a change in .wt/task-1
    fs::write(
        wt.path.join("stripe_client.rs"),
        b"// Stripe client implementation\n",
    )
    .unwrap();
    let add_status = Command::new("git")
        .args(["add", "stripe_client.rs"])
        .current_dir(&wt.path)
        .status()
        .unwrap();
    assert!(add_status.success());
    let commit_status = Command::new("git")
        .args(["commit", "-m", "feat(task-1): add stripe client"])
        .current_dir(&wt.path)
        .status()
        .unwrap();
    assert!(commit_status.success());

    // Merge and cleanup
    wt.merge_and_cleanup("feat(task-1): add stripe client")
        .unwrap();

    // Verify commit and file exist on feat/payment-integration in primary repo
    assert!(
        repo_root.join("stripe_client.rs").exists(),
        "stripe_client.rs should be present on feat/payment-integration"
    );

    let log_output = Command::new("git")
        .args(["log", "-1", "--oneline"])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_str.contains("feat(task-1): add stripe client"),
        "commit should be present in git log of feat/payment-integration"
    );

    // Verify main branch was NOT touched
    let checkout_main = Command::new("git")
        .args(["checkout", "main"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(checkout_main.success());

    assert!(
        !repo_root.join("stripe_client.rs").exists(),
        "stripe_client.rs should NOT exist on main"
    );

    let main_log_output = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let main_log_str = String::from_utf8_lossy(&main_log_output.stdout);
    assert!(
        !main_log_str.contains("stripe client"),
        "main branch must not have milestone commits"
    );
}

#[test]
fn test_worktree_namespacing_on_feature_branch() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    // Create and checkout feature branch
    let status = Command::new("git")
        .args(["checkout", "-b", "feat/auth-v2"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(status.success());

    let manager = WorktreeManager::new(repo_root).unwrap();

    let wt = manager.ensure_task_worktree("task-1", None).unwrap();
    assert_eq!(
        wt.base_branch, "feat/auth-v2",
        "base_branch should match active feature branch"
    );
    assert_eq!(
        wt.branch, "wt/feat-auth-v2-task-1",
        "worktree branch must be namespaced by sanitized feature branch slug"
    );
    assert_eq!(
        wt.path,
        repo_root.join(".wt").join("feat-auth-v2-task-1"),
        "worktree directory must be namespaced by sanitized feature branch slug"
    );
    assert!(wt.path.is_dir(), "worktree directory must exist on disk");
}

#[test]
fn test_concurrent_feature_branch_worktrees() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    // Create two distinct feature branches
    let s1 = Command::new("git")
        .args(["branch", "feat/service-alpha"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(s1.success());

    let s2 = Command::new("git")
        .args(["branch", "feat/service-beta"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(s2.success());

    let manager = WorktreeManager::new(repo_root).unwrap();

    // Both feature branches provision a worktree for task-1 simultaneously
    let wt_alpha = manager
        .ensure_task_worktree("task-1", Some("feat/service-alpha"))
        .expect("Worktree for service-alpha task-1 must provision without error");
    let wt_beta = manager
        .ensure_task_worktree("task-1", Some("feat/service-beta"))
        .expect("Worktree for service-beta task-1 must provision concurrently without error");

    assert_eq!(wt_alpha.branch, "wt/feat-service-alpha-task-1");
    assert_eq!(wt_beta.branch, "wt/feat-service-beta-task-1");
    assert_ne!(
        wt_alpha.path, wt_beta.path,
        "Worktree paths must be distinct and non-colliding"
    );
    assert!(wt_alpha.path.is_dir());
    assert!(wt_beta.path.is_dir());
}

#[test]
fn test_trunk_worktrees_preserve_legacy_names() {
    let repo_dir = init_test_git_repo();
    let repo_root = repo_dir.path();

    let manager = WorktreeManager::new(repo_root).unwrap();

    let wt = manager
        .ensure_task_worktree("task-1", Some("main"))
        .expect("Worktree on main must provision");

    assert_eq!(wt.branch, "wt/task-1");
    assert_eq!(wt.path, repo_root.join(".wt").join("task-1"));
    assert!(wt.path.is_dir());
}
