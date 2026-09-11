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
