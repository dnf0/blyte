use std::fs;
use std::path::Path;
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{enclosing_state_root, save_state_atomic, STATE_DIR};
use superstate_core::{
    branch_scoped_state_path, find_branch_state_for_worktree, sanitize_branch_slug,
};
use tempfile::tempdir;

#[test]
fn test_sanitize_branch_slug() {
    // Normal feature branch with slash
    assert_eq!(
        sanitize_branch_slug("feat/stripe-webhooks"),
        "feat-stripe-webhooks"
    );

    // Branch with issue number and hash
    assert_eq!(sanitize_branch_slug("fix/issue#123"), "fix-issue-123");

    // Multiple consecutive slashes and hyphens collapsed
    assert_eq!(
        sanitize_branch_slug("feature///branch--name"),
        "feature-branch-name"
    );

    // Whitespace and special symbols replaced and collapsed
    assert_eq!(
        sanitize_branch_slug("  feat / auth @ service! $v2  "),
        "feat-auth-service-v2"
    );

    // Leading and trailing hyphens/slashes trimmed
    assert_eq!(sanitize_branch_slug("---feat/test---"), "feat-test");
    assert_eq!(sanitize_branch_slug("/feat/test/"), "feat-test");

    // Characters in [a-zA-Z0-9._-] preserved
    assert_eq!(
        sanitize_branch_slug("release-1.0.0_beta.2"),
        "release-1.0.0_beta.2"
    );

    // Empty or symbol-only branches fallback to "branch"
    assert_eq!(sanitize_branch_slug("///---///"), "branch");
    assert_eq!(sanitize_branch_slug(""), "branch");
}

#[test]
fn test_branch_scoped_state_path() {
    let repo_root = Path::new("/path/to/repo");

    // Feature branch routes to .agent-state/branches/<slug>.json
    assert_eq!(
        branch_scoped_state_path(repo_root, "feat/auth"),
        repo_root
            .join(STATE_DIR)
            .join("branches")
            .join("feat-auth.json")
    );

    // Complex branch slug routes correctly
    assert_eq!(
        branch_scoped_state_path(repo_root, "fix/issue#123"),
        repo_root
            .join(STATE_DIR)
            .join("branches")
            .join("fix-issue-123.json")
    );

    // Default branches route directly to .agent-state/state.json for backwards compatibility
    assert_eq!(
        branch_scoped_state_path(repo_root, "main"),
        repo_root.join(STATE_DIR).join("state.json")
    );
    assert_eq!(
        branch_scoped_state_path(repo_root, "master"),
        repo_root.join(STATE_DIR).join("state.json")
    );
}

#[test]
fn test_find_branch_state_for_worktree() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    let branches_dir = repo_root.join(STATE_DIR).join("branches");
    fs::create_dir_all(&branches_dir).unwrap();

    // Create worktree directories
    let wt1_dir = repo_root.join(".wt/task-1");
    let wt2_dir = repo_root.join(".wt/task-2");
    fs::create_dir_all(&wt1_dir).unwrap();
    fs::create_dir_all(&wt2_dir).unwrap();

    // State 1: has active_worktree pointing to wt1 and active_branch "wt/task-1"
    let state_file_1 = branches_dir.join("feat-payment.json");
    let state_1 = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        active_worktree: Some(wt1_dir.clone()),
        active_branch: Some("wt/task-1".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file_1, &state_1).unwrap();

    // State 2: has active_worktree pointing to wt2 and active_branch "feat/analytics"
    let state_file_2 = branches_dir.join("feat-analytics.json");
    let state_2 = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        active_worktree: Some(wt2_dir.clone()),
        active_branch: Some("feat/analytics".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file_2, &state_2).unwrap();

    // 1. Match by active_worktree path (canonicalized)
    let found_by_wt1 = find_branch_state_for_worktree(repo_root, &wt1_dir, None);
    assert_eq!(found_by_wt1, Some(state_file_1.clone()));

    let found_by_wt2 = find_branch_state_for_worktree(repo_root, &wt2_dir, None);
    assert_eq!(found_by_wt2, Some(state_file_2.clone()));

    // 2. Match by active_branch when worktree does not match
    let non_matching_path = repo_root.join("non_matching_worktree");
    let found_by_branch =
        find_branch_state_for_worktree(repo_root, &non_matching_path, Some("feat/analytics"));
    assert_eq!(found_by_branch, Some(state_file_2));

    // 3. Neither matches
    let not_found =
        find_branch_state_for_worktree(repo_root, &non_matching_path, Some("unknown-branch"));
    assert_eq!(not_found, None);
}

#[test]
fn test_enclosing_state_root_with_branch_scoped_state() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    let branches_dir = repo_root.join(STATE_DIR).join("branches");
    fs::create_dir_all(&branches_dir).unwrap();

    // Deep nested directory inside the repo
    let nested_dir = repo_root.join("crates/subcrate/src/deep");
    fs::create_dir_all(&nested_dir).unwrap();

    // When branches directory is empty (no .json files) and no state.json exists:
    assert_eq!(enclosing_state_root(&nested_dir), None);

    // Create a branch state file in .agent-state/branches/feat-auth.json
    let branch_state = branches_dir.join("feat-auth.json");
    fs::write(&branch_state, "{}").unwrap();

    // Now enclosing_state_root should discover repo_root
    let found = enclosing_state_root(&nested_dir).expect("should find enclosing state root");
    assert_eq!(
        fs::canonicalize(found).unwrap(),
        fs::canonicalize(repo_root).unwrap()
    );
}

#[test]
fn test_resolve_project_root() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    let state_file = repo_root
        .join(STATE_DIR)
        .join("branches")
        .join("feat-payment.json");
    let resolved = superstate_core::resolve_project_root(&state_file);
    assert_eq!(
        fs::canonicalize(resolved).unwrap(),
        fs::canonicalize(repo_root).unwrap()
    );

    let default_state = repo_root.join(STATE_DIR).join("state.json");
    let resolved_default = superstate_core::resolve_project_root(&default_state);
    assert_eq!(
        fs::canonicalize(resolved_default).unwrap(),
        fs::canonicalize(repo_root).unwrap()
    );
}
