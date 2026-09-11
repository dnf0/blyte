use superstate_mcp::server::handle_tool_call;
use tempfile::tempdir;

fn setup_test_repo(branch_name: &str) -> tempfile::TempDir {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let _ = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Superstate Test"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "superstate@test.local"])
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
fn test_mcp_superstate_handoff_with_milestones_array() {
    let temp = setup_test_repo("feat/multi-task-flow");
    let repo_root = temp.path();

    let args = serde_json::json!({
        "milestones": [
            { "title": "T1", "verify_cmd": "true" },
            { "title": "T2", "verify_cmd": "true" }
        ],
        "repo_root": repo_root.to_str().unwrap()
    });

    let res =
        handle_tool_call("superstate_handoff", &args).expect("superstate_handoff should succeed");

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

    // Asserts .agent-state/state.json has base_branch == "feat/multi-task-flow", active_task.id == "task-1", pending_tasks.len() == 1
    let state_file = repo_root.join(".agent-state").join("state.json");
    assert!(state_file.exists(), ".agent-state/state.json must exist");
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

    // Asserts .wt/task-1 exists
    let wt_task1 = repo_root.join(".wt").join("task-1");
    assert!(wt_task1.exists(), ".wt/task-1 should exist on disk");
    assert!(wt_task1.is_dir(), ".wt/task-1 should be a directory");
}

#[test]
fn test_mcp_superstate_preflight_with_verify_cmds() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let args = serde_json::json!({
        "verify_cmds": ["true", "echo hello"],
        "cwd": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("superstate_preflight", &args)
        .expect("superstate_preflight should succeed with verify_cmds");

    assert_eq!(res["executable"], true);
    assert_eq!(res["exit_code"], 0);
    assert_eq!(res["all_passed"], true);

    let results = res["results"]
        .as_array()
        .expect("results array should be present");
    assert_eq!(results.len(), 2);
    for r in results {
        assert_eq!(r["executable"], true);
        assert_eq!(r["exit_code"], 0);
    }
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
        "repo_root": repo_root.to_str().unwrap()
    });
    let _handoff_res = handle_tool_call("superstate_handoff", &handoff_args)
        .expect("superstate_handoff should succeed");

    let wt_task1 = repo_root.join(".wt").join("task-1");
    assert!(wt_task1.exists(), ".wt/task-1 should exist initially");

    // Write an artifact inside .wt/task-1 to ensure it commits and merges
    std::fs::write(wt_task1.join("task1_artifact.txt"), "task 1 done").unwrap();

    // 2. Call state_next
    let next_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let next_res = handle_tool_call("state_next", &next_args).expect("state_next should succeed");

    // Asserts .wt/task-1 is merged to feat/multi-task-flow and cleaned up
    assert!(
        !wt_task1.exists(),
        ".wt/task-1 should be cleaned up after state_next"
    );
    assert!(
        repo_root.join("task1_artifact.txt").exists(),
        "task1_artifact.txt should be merged into feat/multi-task-flow"
    );

    // Verify git branch in repo_root is still feat/multi-task-flow
    let branch_out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_root)
        .output()
        .expect("git rev-parse");
    let branch_str = String::from_utf8_lossy(&branch_out.stdout)
        .trim()
        .to_string();
    assert_eq!(
        branch_str, "feat/multi-task-flow",
        "Primary repo should be on feat/multi-task-flow"
    );

    // Asserts .wt/task-2 is provisioned
    let wt_task2 = repo_root.join(".wt").join("task-2");
    assert!(
        wt_task2.exists(),
        ".wt/task-2 should be provisioned on disk"
    );
    assert!(wt_task2.is_dir(), ".wt/task-2 should be a directory");

    // Asserts active_task.id is task-2
    assert_eq!(
        next_res["active_task"]["id"], "task-2",
        "active_task.id should be task-2 in response"
    );

    // Also verify state.json on disk
    let state_file = repo_root.join(".agent-state").join("state.json");
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state_json: serde_json::Value = serde_json::from_str(&state_content).unwrap();
    assert_eq!(
        state_json["active_task"]["id"], "task-2",
        "active_task.id in state.json should be task-2"
    );
    assert_eq!(
        state_json["active_worktree"],
        wt_task2.display().to_string(),
        "active_worktree in state.json should point to task-2"
    );

    // Asserts response contains turnkey prompt for task-2
    let prompt = next_res["turnkey_prompt"]
        .as_str()
        .or_else(|| {
            next_res["content"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
        })
        .expect("turnkey prompt for task-2 should be present in response");
    assert!(
        prompt.contains("task-2"),
        "Turnkey prompt must reference 'task-2', got: {}",
        prompt
    );
}
