use superstate_mcp::server::{handle_json_rpc, JsonRpcRequest};
use superstate_mcp::tools::list_all_mcp_tools;

#[test]
fn test_all_cli_parity_tools_declared() {
    let tools = list_all_mcp_tools();
    let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

    let expected = [
        "state_init",
        "state_show",
        "state_status",
        "state_next",
        "state_ready",
        "state_checkpoint",
        "state_rollback",
        "state_prompt",
        "state_sync",
        "state_exec",
        "state_doctor",
        "state_validate",
        "state_apply",
        "state_resume",
        "state_metrics",
        "state_list",
        "state_note",
        "worktree_create",
        "worktree_merge",
        "worktree_list",
        "worktree_remove",
        "worktree_clean",
        "superstate_review",
        "superstate_preflight",
        "superstate_handoff",
    ];

    for name in expected {
        assert!(
            names.contains(&name.to_string()),
            "Tool '{}' missing from list_all_mcp_tools()",
            name
        );
    }

    assert_eq!(
        tools.len(),
        25,
        "Expected exactly 25 tools, found {}",
        tools.len()
    );
}

#[test]
fn test_tool_parameter_schemas_and_requirements() {
    let tools = list_all_mcp_tools();
    let find_tool = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("Tool '{}' not found", name))
    };

    // state_next schema
    let state_next = find_tool("state_next");
    let next_props = &state_next.input_schema.properties;
    assert_eq!(
        next_props["ignore_untracked_glob"]["type"], "string",
        "state_next must define ignore_untracked_glob of type string"
    );
    assert_eq!(
        next_props["verify_cmd"]["type"], "string",
        "state_next must define verify_cmd of type string"
    );
    assert_eq!(
        next_props["verify_git_clean"]["type"], "boolean",
        "state_next must define verify_git_clean of type boolean"
    );

    // state_prompt schema
    let state_prompt = find_tool("state_prompt");
    let prompt_props = &state_prompt.input_schema.properties;
    assert_eq!(
        prompt_props["with_graph"]["type"], "boolean",
        "state_prompt must define with_graph of type boolean"
    );
    assert_eq!(
        prompt_props["task"]["type"], "string",
        "state_prompt must define task of type string"
    );

    // state_exec schema
    let state_exec = find_tool("state_exec");
    assert!(
        state_exec
            .input_schema
            .required
            .contains(&"verify_cmd".to_string()),
        "state_exec must require verify_cmd"
    );
    let exec_props = &state_exec.input_schema.properties;
    assert_eq!(exec_props["verify_cmd"]["type"], "string");
    assert_eq!(exec_props["state"]["type"], "string");
    assert_eq!(exec_props["verify_git_clean"]["type"], "boolean");
    assert_eq!(exec_props["checkpoint"]["type"], "boolean");
    assert_eq!(exec_props["rollback_on_fail"]["type"], "boolean");
    assert_eq!(exec_props["advance"]["type"], "boolean");

    // worktree_remove schema
    let wt_remove = find_tool("worktree_remove");
    assert!(
        wt_remove
            .input_schema
            .required
            .contains(&"branch".to_string()),
        "worktree_remove must require branch"
    );
    let remove_props = &wt_remove.input_schema.properties;
    assert_eq!(remove_props["branch"]["type"], "string");
    assert_eq!(remove_props["force"]["type"], "boolean");
    assert_eq!(remove_props["repo"]["type"], "string");

    // state_sync schema
    let state_sync = find_tool("state_sync");
    assert!(
        state_sync
            .input_schema
            .required
            .contains(&"plan".to_string()),
        "state_sync must require plan"
    );
    let sync_props = &state_sync.input_schema.properties;
    assert_eq!(sync_props["plan"]["type"], "string");
    assert_eq!(sync_props["state"]["type"], "string");
    assert_eq!(sync_props["out"]["type"], "string");

    // state_show schema
    let state_show = find_tool("state_show");
    let show_props = &state_show.input_schema.properties;
    assert_eq!(show_props["state"]["type"], "string");
    assert_eq!(show_props["json"]["type"], "boolean");

    // state_status schema
    let state_status = find_tool("state_status");
    let status_props = &state_status.input_schema.properties;
    assert_eq!(status_props["state"]["type"], "string");
    assert_eq!(status_props["json"]["type"], "boolean");

    // state_resume schema
    let state_resume = find_tool("state_resume");
    let resume_props = &state_resume.input_schema.properties;
    assert_eq!(resume_props["state"]["type"], "string");

    // state_metrics schema
    let state_metrics = find_tool("state_metrics");
    let metrics_props = &state_metrics.input_schema.properties;
    assert_eq!(metrics_props["state"]["type"], "string");
    assert_eq!(metrics_props["json"]["type"], "boolean");

    // state_checkpoint schema
    let state_checkpoint = find_tool("state_checkpoint");
    let checkpoint_props = &state_checkpoint.input_schema.properties;
    assert_eq!(checkpoint_props["state"]["type"], "string");

    // state_rollback schema
    let state_rollback = find_tool("state_rollback");
    let rollback_props = &state_rollback.input_schema.properties;
    assert_eq!(rollback_props["state"]["type"], "string");
    assert_eq!(rollback_props["reference"]["type"], "string");

    // state_ready schema
    let state_ready = find_tool("state_ready");
    let ready_props = &state_ready.input_schema.properties;
    assert_eq!(ready_props["state"]["type"], "string");
    assert_eq!(ready_props["json"]["type"], "boolean");

    // state_doctor schema
    let state_doctor = find_tool("state_doctor");
    let doctor_props = &state_doctor.input_schema.properties;
    assert_eq!(doctor_props["state"]["type"], "string");
    assert_eq!(doctor_props["plan"]["type"], "string");
    assert_eq!(doctor_props["repair"]["type"], "boolean");
    assert_eq!(doctor_props["json"]["type"], "boolean");

    // state_validate schema
    let state_validate = find_tool("state_validate");
    let validate_props = &state_validate.input_schema.properties;
    assert_eq!(validate_props["state"]["type"], "string");

    // state_apply schema
    let state_apply = find_tool("state_apply");
    let apply_props = &state_apply.input_schema.properties;
    assert_eq!(apply_props["state"]["type"], "string");
    assert_eq!(apply_props["patch"]["type"], "string");
    assert_eq!(apply_props["note"]["type"], "string");
    assert_eq!(apply_props["out"]["type"], "string");

    // state_list schema
    let state_list = find_tool("state_list");
    let list_props = &state_list.input_schema.properties;
    assert_eq!(list_props["state"]["type"], "string");
    assert_eq!(list_props["json"]["type"], "boolean");

    // worktree_list schema
    let worktree_list = find_tool("worktree_list");
    let wt_list_props = &worktree_list.input_schema.properties;
    assert_eq!(wt_list_props["repo"]["type"], "string");

    // worktree_clean schema
    let worktree_clean = find_tool("worktree_clean");
    let wt_clean_props = &worktree_clean.input_schema.properties;
    assert_eq!(wt_clean_props["repo"]["type"], "string");
}

#[test]
fn test_tools_list_rpc_schema_parity() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: "tools/list".to_string(),
        params: None,
    };
    let resp = handle_json_rpc(req);
    assert!(resp.error.is_none());
    let result = resp.result.expect("result should be present");
    let tools = result
        .get("tools")
        .and_then(|v| v.as_array())
        .expect("tools array");
    assert_eq!(tools.len(), 25);
}
