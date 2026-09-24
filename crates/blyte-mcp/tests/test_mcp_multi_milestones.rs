use blyte_mcp::server::handle_tool_call;
use tempfile::tempdir;

fn setup_test_repo(branch_name: &str) -> tempfile::TempDir {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let _ = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Blyte Test"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "blyte@test.local"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args([
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "chore: baseline",
        ])
        .current_dir(repo_root)
        .status();
    if branch_name != "main" {
        let _ = std::process::Command::new("git")
            .args(["checkout", "-b", branch_name])
            .current_dir(repo_root)
            .status();
    }
    temp
}

#[test]
fn test_mcp_blyte_handoff_with_milestones_array() {
    let temp = setup_test_repo("feat/multi-task-flow");
    let repo_root = temp.path();

    let args = serde_json::json!({
        "milestones": [
            { "title": "T1", "verify_cmd": "true" },
            { "title": "T2", "verify_cmd": "true" }
        ],
        "worktree": true,
        "detach": false,
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("blyte_handoff", &args).expect("blyte_handoff should succeed");

    // Asserts response contains task-1 turnkey prompt
    let prompt = res["turnkey_prompt"]
        .as_str()
        .or_else(|| {
            res["content"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
        })
        .expect("turnkey prompt should be present in response");
    assert!(
        prompt.contains("task-1"),
        "Turnkey prompt must contain 'task-1', got: {}",
        prompt
    );

    // Asserts .agent-state/branches/feat-multi-task-flow.json has base_branch == "feat/multi-task-flow", active_task.id == "task-1", pending_tasks.len() == 1
    let state_file = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-multi-task-flow.json");
    assert!(
        state_file.exists(),
        ".agent-state/branches/feat-multi-task-flow.json must exist"
    );
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state_json: serde_json::Value = serde_json::from_str(&state_content).unwrap();

    assert_eq!(
        state_json["base_branch"], "feat/multi-task-flow",
        "base_branch should be 'feat/multi-task-flow'"
    );
    assert_eq!(
        state_json["active_task"]["id"], "task-1",
        "active_task.id should be 'task-1'"
    );
    let pending = state_json["pending_tasks"]
        .as_array()
        .expect("pending_tasks should be array");
    assert_eq!(pending.len(), 1, "pending_tasks should have 1 item");
    assert_eq!(
        pending[0]["id"], "task-2",
        "pending task should be 'task-2'"
    );

    // Asserts .wt/feat-multi-task-flow-task-1 exists
    let wt_task1 = repo_root.join(".wt").join("feat-multi-task-flow-task-1");
    assert!(
        wt_task1.exists(),
        ".wt/feat-multi-task-flow-task-1 should exist on disk"
    );
    assert!(
        wt_task1.is_dir(),
        ".wt/feat-multi-task-flow-task-1 should be a directory"
    );
}

#[test]
fn test_mcp_blyte_preflight_with_verify_cmds() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let args = serde_json::json!({
        "verify_cmds": ["true", "echo hello"],
        "cwd": repo_root.to_str().unwrap()
    });

    let err = handle_tool_call("blyte_preflight", &args).unwrap_err();
    assert!(err.contains("deprecated"));
    assert!(err.contains("blyte_handoff"));
}

#[test]
fn test_mcp_state_next_advances_and_provisions_task_2() {
    let temp = setup_test_repo("feat/multi-task-flow");
    let repo_root = temp.path();

    // 1. Initialize 2 milestones
    let handoff_args = serde_json::json!({
        "milestones": [
            { "title": "T1", "verify_cmd": "true" },
            { "title": "T2", "verify_cmd": "true" }
        ],
        "worktree": true,
        "repo_root": repo_root.to_str().unwrap()
    });
    let _handoff_res =
        handle_tool_call("blyte_handoff", &handoff_args).expect("blyte_handoff should succeed");

    let wt_task1 = repo_root.join(".wt").join("feat-multi-task-flow-task-1");
    assert!(
        wt_task1.exists(),
        ".wt/feat-multi-task-flow-task-1 should exist initially"
    );

    // Write an artifact inside .wt/feat-multi-task-flow-task-1 to ensure it commits and merges
    std::fs::write(wt_task1.join("task1_artifact.txt"), "task 1 done").unwrap();

    // 2. Call state_next and verify deprecation error
    let next_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let next_err = handle_tool_call("state_next", &next_args).unwrap_err();
    assert!(next_err.contains("deprecated"));
    assert!(next_err.contains("blyte_handoff"));
}
