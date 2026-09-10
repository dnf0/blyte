//! Integration tests for worktree lifecycle integration in `state next` and `state rollback`.

use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_cli::commands::state::{
    cmd_checkpoint, cmd_init, cmd_next, cmd_prompt, cmd_rollback,
};
use superstate_core::state_machine::load_state;
use tempfile::tempdir;

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
fn test_state_next_worktree_merge_and_advance() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    let plan_file = repo_root.join("plan.md");
    fs::write(
        &plan_file,
        "### Task 1: Add feature\n- **Depends on:** none\n- Files to modify: feature.txt\n\n\
         ### Task 2: Second task\n- **Depends on:** Task 1\n- Files to modify: second.txt\n",
    )
    .unwrap();

    let state_file = repo_root.join(".agent-state").join("state.json");
    cmd_init(plan_file, state_file.clone()).expect("Failed to initialize state");

    // 1. Call cmd_prompt to provision worktree for Task 1
    cmd_prompt(&state_file, None, false, false).expect("cmd_prompt failed");

    let expected_wt_path = repo_root.join(".wt").join("task-1");
    assert!(
        expected_wt_path.exists(),
        "Worktree directory .wt/task-1 should exist"
    );

    let state_before = load_state(&state_file).expect("Failed to load state before next");
    assert_eq!(state_before.active_worktree, Some(expected_wt_path.clone()));
    assert_eq!(
        state_before.active_branch,
        Some("wt/task-1".to_string())
    );

    // 2. Modify a file and commit inside .wt/task-1
    let feature_file_in_wt = expected_wt_path.join("feature.txt");
    fs::write(
        &feature_file_in_wt,
        b"feature implementation inside worktree\n",
    )
    .unwrap();

    let add_status = Command::new("git")
        .args(["add", "feature.txt"])
        .current_dir(&expected_wt_path)
        .status()
        .unwrap();
    assert!(add_status.success());

    let commit_status = Command::new("git")
        .args(["commit", "-m", "feat: implement feature in worktree"])
        .current_dir(&expected_wt_path)
        .status()
        .unwrap();
    assert!(commit_status.success());

    // Before cmd_next, feature.txt should NOT exist in repo_root (primary checkout)
    assert!(
        !repo_root.join("feature.txt").exists(),
        "feature.txt must NOT exist in main repo before merge"
    );

    // 3. Call cmd_next with verify_cmd checking feature.txt and verify_git_clean = true
    // This asserts that verification runs inside the worktree (where feature.txt exists).
    cmd_next(&state_file, Some("test -f feature.txt".to_string()), true)
        .expect("cmd_next should succeed and merge worktree");

    // 4. Assert worktree changes were merged into main
    let feature_file_in_main = repo_root.join("feature.txt");
    assert!(
        feature_file_in_main.exists(),
        "feature.txt should now exist on main branch in primary repo after merge"
    );
    let content = fs::read_to_string(&feature_file_in_main).unwrap();
    assert_eq!(content, "feature implementation inside worktree\n");

    // 5. Assert .wt/task-1 was deleted
    assert!(
        !expected_wt_path.exists(),
        ".wt/task-1 should be deleted after successful merge and cleanup"
    );

    // 6. Assert state advanced to Task 2, active_worktree is None, active_branch is None
    let state_after = load_state(&state_file).expect("Failed to load state after next");
    assert_eq!(
        state_after.active_worktree, None,
        "state.active_worktree must be cleared after merge"
    );
    assert_eq!(
        state_after.active_branch, None,
        "state.active_branch must be cleared after merge"
    );
    assert_eq!(state_after.step, 2, "step should advance from 1 to 2");
    let active_task = state_after
        .active_task
        .expect("Task 2 should now be active");
    assert_eq!(active_task.id, "Task 2");
}

#[test]
fn test_state_next_preserves_worktree_on_failure() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    let plan_file = repo_root.join("plan.md");
    fs::write(
        &plan_file,
        "### Task 1: First task\n- **Depends on:** none\n- Files to modify: task1.txt\n\n\
         ### Task 2: Second task\n- **Depends on:** Task 1\n",
    )
    .unwrap();

    let state_file = repo_root.join(".agent-state").join("state.json");
    cmd_init(plan_file, state_file.clone()).expect("Failed to initialize state");

    // Provision worktree for Task 1
    cmd_prompt(&state_file, None, false, false).expect("cmd_prompt failed");

    let expected_wt_path = repo_root.join(".wt").join("task-1");
    assert!(expected_wt_path.exists());

    // Commit a file inside worktree to keep worktree git status clean
    fs::write(
        expected_wt_path.join("task1.txt"),
        b"some work in progress\n",
    )
    .unwrap();
    let add_status = Command::new("git")
        .args(["add", "task1.txt"])
        .current_dir(&expected_wt_path)
        .status()
        .unwrap();
    assert!(add_status.success());
    let commit_status = Command::new("git")
        .args(["commit", "-m", "feat: wip in worktree"])
        .current_dir(&expected_wt_path)
        .status()
        .unwrap();
    assert!(commit_status.success());

    // Call cmd_next with a failing verify_cmd
    let res = cmd_next(&state_file, Some("exit 1".to_string()), true);
    assert!(res.is_err(), "cmd_next must return Err when gate fails");

    // Assert .wt/task-1 STILL exists
    assert!(
        expected_wt_path.exists(),
        "Worktree directory must be preserved on verification failure"
    );

    // Reload state and verify error recorded and step did NOT advance
    let state = load_state(&state_file).expect("Failed to reload state");
    assert!(
        !state.errors.is_empty(),
        "state.errors must record the failure message"
    );
    assert!(
        state
            .errors
            .iter()
            .any(|e| e.contains("1") || e.contains("verify_cmd")),
        "errors should contain verification failure: {:?}",
        state.errors
    );
    assert_eq!(
        state.step, 1,
        "state.step must NOT advance when verification fails"
    );
    assert_eq!(
        state.active_task.as_ref().map(|t| t.id.as_str()),
        Some("Task 1"),
        "Task 1 must remain active"
    );
    assert_eq!(
        state.active_worktree,
        Some(expected_wt_path),
        "state.active_worktree must remain intact"
    );
}

#[test]
fn test_state_rollback_cleans_up_worktree() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    let plan_file = repo_root.join("plan.md");
    fs::write(
        &plan_file,
        "### Task 1: First task\n- **Depends on:** none\n- Files to modify: task1.txt\n",
    )
    .unwrap();

    let state_file = repo_root.join(".agent-state").join("state.json");
    cmd_init(plan_file, state_file.clone()).expect("Failed to initialize state");
    cmd_checkpoint(&state_file).expect("Failed to create checkpoint");

    // Provision worktree for Task 1
    cmd_prompt(&state_file, None, false, false).expect("cmd_prompt failed");

    let expected_wt_path = repo_root.join(".wt").join("task-1");
    assert!(expected_wt_path.exists());

    // Create a file inside worktree
    fs::write(expected_wt_path.join("task1.txt"), b"dirty content").unwrap();

    // Call cmd_rollback
    cmd_rollback(&state_file, None).expect("cmd_rollback should succeed");

    // Worktree should be cleanly aborted / destroyed
    assert!(
        !expected_wt_path.exists(),
        "Worktree directory should be deleted on rollback"
    );

    // Reload state and verify worktree tracking fields are reset
    let state = load_state(&state_file).expect("Failed to reload state");
    assert_eq!(
        state.active_worktree, None,
        "state.active_worktree should be None after rollback"
    );
    assert_eq!(
        state.active_branch, None,
        "state.active_branch should be None after rollback"
    );
}
