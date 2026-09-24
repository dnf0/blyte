//! Integration tests for branch-scoped state isolation and multi-agent concurrency in MCP.

use blyte_core::models::ExecutionStatus;
use blyte_core::state_machine::load_state;
use blyte_mcp::server::handle_tool_call;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn init_git_repo(path: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    };

    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Test User"]);
    run(&["config", "user.email", "test@example.com"]);

    fs::write(path.join("README.md"), b"# Base\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "--no-gpg-sign", "-m", "chore: initial baseline"]);
}

#[test]
fn test_mcp_handoff_creates_branch_scoped_state() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    // Create and checkout feature branch feat/service-a
    let status = Command::new("git")
        .args(["checkout", "-b", "feat/service-a"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(status.success());

    // Call blyte_handoff without "state"
    let args = serde_json::json!({
        "goal": "Build service A",
        "verify_cmd": "echo 'service-a ok'",
        "no_worktree": true,
        "detach": false,
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("blyte_handoff", &args).expect("blyte_handoff should succeed");
    assert_eq!(res["status"], "ready");

    let branch_state = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-service-a.json");
    let legacy_state = repo_root.join(".agent-state").join("state.json");

    assert!(
        branch_state.exists(),
        "Branch-scoped state file .agent-state/branches/feat-service-a.json must exist"
    );
    assert!(
        !legacy_state.exists(),
        "Legacy .agent-state/state.json must NOT exist on feature branch"
    );
}

#[test]
fn test_mcp_concurrent_agents_independent_states() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    // Create branches feat/service-a and feat/service-b
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    };
    run(&["branch", "feat/service-a"]);
    run(&["branch", "feat/service-b"]);

    // Agent A calls blyte_handoff on branch feat/service-a
    run(&["checkout", "feat/service-a"]);
    let args_a = serde_json::json!({
        "goal": "Implement Service A",
        "verify_cmd": "echo 'Service A ok'",
        "no_worktree": true,
        "detach": false,
        "repo_root": repo_root.to_str().unwrap()
    });
    let res_a =
        handle_tool_call("blyte_handoff", &args_a).expect("Agent A blyte_handoff should succeed");
    assert_eq!(res_a["status"], "ready");

    // Agent B calls blyte_handoff on branch feat/service-b
    run(&["checkout", "feat/service-b"]);
    let args_b = serde_json::json!({
        "goal": "Implement Service B",
        "verify_cmd": "echo 'Service B ok'",
        "no_worktree": true,
        "detach": false,
        "repo_root": repo_root.to_str().unwrap()
    });
    let res_b =
        handle_tool_call("blyte_handoff", &args_b).expect("Agent B blyte_handoff should succeed");
    assert_eq!(res_b["status"], "ready");

    let state_file_a = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-service-a.json");
    let state_file_b = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-service-b.json");
    let legacy_state = repo_root.join(".agent-state").join("state.json");

    assert!(
        state_file_a.exists(),
        "Agent A state file must exist independently at .agent-state/branches/feat-service-a.json"
    );
    assert!(
        state_file_b.exists(),
        "Agent B state file must exist independently at .agent-state/branches/feat-service-b.json"
    );
    assert!(
        !legacy_state.exists(),
        "Legacy .agent-state/state.json must NOT exist"
    );

    // Verify initial states
    let state_a_initial = load_state(&state_file_a).expect("load state A");
    let state_b_initial = load_state(&state_file_b).expect("load state B");
    assert_eq!(state_a_initial.step, 1);
    assert_eq!(state_b_initial.step, 1);
    assert_eq!(state_a_initial.status, ExecutionStatus::InProgress);
    assert_eq!(state_b_initial.status, ExecutionStatus::InProgress);

    // Agent A calls state_next on branch feat/service-a and verifies deprecation error
    run(&["checkout", "feat/service-a"]);
    let next_args_a = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let next_err_a = handle_tool_call("state_next", &next_args_a).unwrap_err();
    assert!(next_err_a.contains("deprecated"));
    assert!(next_err_a.contains("blyte_handoff"));
}

#[test]
fn test_mcp_state_get_and_step_resolve_branch_scoped_state() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    };

    run(&["checkout", "-b", "feat/calc-engine"]);

    let handoff_args = serde_json::json!({
        "goal": "Build calc engine",
        "verify_cmd": "true",
        "no_worktree": true,
        "detach": false,
        "repo_root": repo_root.to_str().unwrap()
    });
    let handoff_res =
        handle_tool_call("blyte_handoff", &handoff_args).expect("blyte_handoff should succeed");
    assert_eq!(handoff_res["status"], "ready");

    // Calling legacy state_show / state_next returns deprecation error
    let get_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let get_err = handle_tool_call("state_show", &get_args).unwrap_err();
    assert!(get_err.contains("deprecated"));

    let next_err = handle_tool_call("state_next", &get_args).unwrap_err();
    assert!(next_err.contains("deprecated"));
}
