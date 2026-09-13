use serde_json::json;
use std::path::PathBuf;
use superstate_mcp::server::handle_tool_call;
use tempfile::tempdir;

fn setup_test_repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();

    let _ = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test Agent"])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@agent.local"])
        .current_dir(&repo_root)
        .output();

    let readme = repo_root.join("README.md");
    std::fs::write(&readme, "# Test Repo\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(&repo_root)
        .output();

    (temp, repo_root)
}

#[test]
fn test_worktree_merge_blocks_on_failed_verification_and_keeps_worktree_intact() {
    let (_dir, repo_root) = setup_test_repo();

    let create_res = handle_tool_call(
        "worktree_create",
        &json!({
            "repo_root": repo_root.to_str().unwrap(),
            "branch": "wt/task-gate-fail"
        }),
    )
    .expect("worktree_create should succeed");

    let wt_path = PathBuf::from(create_res["worktree_path"].as_str().unwrap());
    assert!(wt_path.exists(), "worktree path must exist");

    // Add a file in the worktree
    let feature_file = wt_path.join("feature.txt");
    std::fs::write(&feature_file, "worktree content").unwrap();

    // Call worktree_merge with failing verification command
    let merge_res = handle_tool_call(
        "worktree_merge",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "branch": "wt/task-gate-fail",
            "verify_full_suite": "echo 'verification failed output' >&2 && exit 2"
        }),
    );

    let err =
        merge_res.expect_err("worktree_merge should fail when verify_full_suite exits non-zero");
    assert!(
        err.contains("Pre-merge verification failed (exit code 2)"),
        "error should report pre-merge verification failure: {}",
        err
    );
    assert!(
        err.contains("verification failed output"),
        "error should contain stderr output: {}",
        err
    );

    // Verify worktree directory is intact
    assert!(
        wt_path.exists(),
        "worktree directory must remain intact after verification failure"
    );

    // Verify worktree is still listed in worktree_list
    let list_res = handle_tool_call(
        "worktree_list",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "json": true
        }),
    )
    .expect("worktree_list should succeed");

    let branches: Vec<&str> = list_res["worktrees"]
        .as_array()
        .expect("worktrees array")
        .iter()
        .filter_map(|w| w["branch"].as_str())
        .collect();
    assert!(
        branches.contains(&"wt/task-gate-fail"),
        "worktree should still be present in worktree_list"
    );

    // Verify changes were NOT merged into the base branch
    assert!(
        !repo_root.join("feature.txt").exists(),
        "base branch should not contain worktree changes when verification fails"
    );
}

#[test]
fn test_worktree_merge_succeeds_when_verification_passes() {
    let (_dir, repo_root) = setup_test_repo();

    let create_res = handle_tool_call(
        "worktree_create",
        &json!({
            "repo_root": repo_root.to_str().unwrap(),
            "branch": "wt/task-gate-pass"
        }),
    )
    .expect("worktree_create should succeed");

    let wt_path = PathBuf::from(create_res["worktree_path"].as_str().unwrap());
    assert!(wt_path.exists());

    let pass_file = wt_path.join("pass.txt");
    std::fs::write(&pass_file, "pass content").unwrap();

    let merge_res = handle_tool_call(
        "worktree_merge",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "branch": "wt/task-gate-pass",
            "verify_full_suite": "echo 'verification passed'"
        }),
    )
    .expect("worktree_merge should succeed when verification passes");

    assert_eq!(merge_res["status"], "success");
    assert_eq!(merge_res["branch"], "wt/task-gate-pass");

    // Verify changes were merged into the base branch
    assert!(
        repo_root.join("pass.txt").exists(),
        "base branch should contain worktree changes after successful merge"
    );
}
