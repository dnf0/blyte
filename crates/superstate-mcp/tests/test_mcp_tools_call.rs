use superstate_mcp::server::{handle_json_rpc, JsonRpcRequest};
use tempfile::tempdir;

#[test]
fn test_tools_list_contains_preflight_and_handoff() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: "tools/list".to_string(),
        params: None,
    };
    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/list returned error: {:?}",
        resp.error
    );
    let result = resp.result.expect("result should be present");
    let tools = result
        .get("tools")
        .and_then(|v| v.as_array())
        .expect("tools array");
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();

    assert!(
        tool_names.contains(&"superstate_preflight"),
        "tools/list must contain superstate_preflight"
    );
    assert!(
        tool_names.contains(&"superstate_handoff"),
        "tools/list must contain superstate_handoff"
    );
}

#[test]
fn test_tools_call_superstate_preflight_success() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "superstate_preflight",
            "arguments": {
                "verify_cmd": "echo 'hello preflight'"
            }
        })),
    };

    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/call preflight returned error: {:?}",
        resp.error
    );
    let res = resp.result.expect("result should be present");
    assert_eq!(res["executable"], true);
    assert_eq!(res["exit_code"], 0);
    assert!(
        res["stdout"]
            .as_str()
            .unwrap_or("")
            .contains("hello preflight"),
        "stdout should contain 'hello preflight', got: {:?}",
        res["stdout"]
    );
}

#[test]
fn test_tools_call_superstate_preflight_invalid_command() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(3)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "superstate_preflight",
            "arguments": {
                "verify_cmd": "non_existent_command_xyz_123"
            }
        })),
    };

    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/call preflight returned error: {:?}",
        resp.error
    );
    let res = resp.result.expect("result should be present");
    let executable = res
        .get("executable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let exit_code = res.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert!(
        !executable || exit_code != 0,
        "expected executable: false or exit_code != 0, got executable={}, exit_code={}",
        executable,
        exit_code
    );
}

#[test]
fn test_tools_call_superstate_handoff() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    // Initialize minimal git baseline
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

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(4)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "superstate_handoff",
            "arguments": {
                "goal": "Fix bug in auth",
                "verify_cmd": "echo 'ok'",
                "no_worktree": true,
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };

    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/call handoff returned error: {:?}",
        resp.error
    );
    let res = resp.result.expect("result should be present");

    assert_eq!(res["status"], "ready");
    assert_eq!(res["task_id"], "task-1");
    let prompt = res["turnkey_prompt"]
        .as_str()
        .expect("turnkey_prompt string");
    assert!(
        prompt.contains("Hard stop upon passing") || prompt.contains("Task: Fix bug in auth"),
        "turnkey prompt must contain task info or hard stop invariant, got: {}",
        prompt
    );

    // Verify .agent-state/state.json was created on disk
    let state_file = repo_root.join(".agent-state").join("state.json");
    assert!(state_file.exists(), ".agent-state/state.json must exist");

    // Verify NO plan.md was created on disk
    let plan_file = repo_root.join("plan.md");
    assert!(!plan_file.exists(), "plan.md must NOT exist on disk");
}

#[test]
fn test_tools_call_superstate_preflight_failing_command() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(5)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "superstate_preflight",
            "arguments": {
                "verify_cmd": "test 1 -eq 2"
            }
        })),
    };

    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/call preflight returned error: {:?}",
        resp.error
    );
    let res = resp.result.expect("result should be present");
    assert_eq!(
        res["executable"], true,
        "executable must be true for a failing command that ran to completion"
    );
    assert_eq!(res["exit_code"], 1, "exit_code must be 1 for test 1 -eq 2");
}

#[test]
fn test_tools_call_superstate_handoff_with_worktree() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    // Initialize baseline git repo
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

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(6)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "superstate_handoff",
            "arguments": {
                "goal": "Implement feature in worktree",
                "verify_cmd": "echo 'verified'",
                "no_worktree": false,
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };

    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/call handoff returned error: {:?}",
        resp.error
    );
    let res = resp.result.expect("result should be present");

    assert_eq!(res["status"], "ready");
    assert_eq!(res["task_id"], "task-1");

    let wt_path = repo_root.join(".wt").join("task-1");
    assert!(
        wt_path.exists(),
        "Worktree directory .wt/task-1 should exist on disk"
    );
    assert!(
        wt_path.is_dir(),
        "Worktree directory .wt/task-1 should be a directory"
    );

    let prompt = res["turnkey_prompt"]
        .as_str()
        .expect("turnkey_prompt string");
    assert!(
        prompt.contains(".wt/task-1"),
        "Prompt should reference worktree path: {}",
        prompt
    );
    assert!(
        prompt.contains("wt/task-1"),
        "Prompt should reference worktree branch: {}",
        prompt
    );

    // Verify git branch wt/task-1 exists
    let branch_out = std::process::Command::new("git")
        .args(["branch"])
        .current_dir(repo_root)
        .output()
        .expect("git branch");
    let branch_str = String::from_utf8_lossy(&branch_out.stdout);
    assert!(
        branch_str.contains("wt/task-1"),
        "git branch output should contain wt/task-1, got: {}",
        branch_str
    );
}

#[test]
fn test_tools_call_state_next_with_verification_and_merge() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    // Initialize baseline git repo
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

    // 1. Initialize handoff with worktree and a verification command checking for file created
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(7)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "superstate_handoff",
            "arguments": {
                "goal": "Write artifact in worktree",
                "verify_cmd": "test -f artifact.txt",
                "no_worktree": false,
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };
    let resp = handle_json_rpc(req);
    assert!(resp.error.is_none());

    let wt_path = repo_root.join(".wt").join("task-1");
    assert!(wt_path.exists());

    // 2. Call state_next before creating artifact.txt -> gate should fail!
    let next_req_fail = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(8)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "state_next",
            "arguments": {
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };
    let next_resp_fail = handle_json_rpc(next_req_fail);
    assert!(
        next_resp_fail.error.is_some(),
        "state_next should fail verification when artifact.txt does not exist"
    );

    // 3. Create artifact.txt inside the worktree
    std::fs::write(wt_path.join("artifact.txt"), "hello from worktree").unwrap();

    // 4. Call state_next -> gate should pass and worktree merged into base
    let next_req_pass = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(9)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "state_next",
            "arguments": {
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };
    let next_resp_pass = handle_json_rpc(next_req_pass);
    assert!(
        next_resp_pass.error.is_none(),
        "state_next should pass when gate succeeds: {:?}",
        next_resp_pass.error
    );

    // Verify artifact.txt is now present in the primary repo (merged into main)
    assert!(
        repo_root.join("artifact.txt").exists(),
        "artifact.txt should be merged into primary repo base branch"
    );

    // Verify worktree has been cleaned up
    assert!(
        !wt_path.exists(),
        "worktree .wt/task-1 should be cleaned up after merge"
    );
}

#[test]
fn test_json_rpc_error_cases() {
    // 1. Unknown method
    let req_unknown_method = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(10)),
        method: "unknown_method_xyz".to_string(),
        params: None,
    };
    let resp = handle_json_rpc(req_unknown_method);
    assert!(resp.result.is_none());
    assert!(resp.error.is_some());
    let err = resp.error.unwrap();
    assert_eq!(err["code"], -32601);
    assert!(err["message"]
        .as_str()
        .unwrap()
        .contains("Method not found"));

    // 2. Unknown tool in tools/call
    let req_unknown_tool = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(11)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "non_existent_tool_abc",
            "arguments": {}
        })),
    };
    let resp = handle_json_rpc(req_unknown_tool);
    assert!(resp.result.is_none());
    assert!(resp.error.is_some());
    let err = resp.error.unwrap();
    assert_eq!(err["code"], -32603);
    assert!(err["message"]
        .as_str()
        .unwrap()
        .contains("Unknown tool: non_existent_tool_abc"));
}

#[test]
fn test_execute_preflight_timeout() {
    use std::time::Duration;
    use superstate_mcp::server::execute_preflight_with_timeout;

    let temp = tempdir().unwrap();
    let res = execute_preflight_with_timeout("sleep 2", temp.path(), Duration::from_millis(100));
    assert!(res.is_err(), "command sleep 2 should time out after 100ms");
    let err = res.err().unwrap();
    assert!(
        err.contains("timed out"),
        "error should indicate timeout, got: {}",
        err
    );
}
