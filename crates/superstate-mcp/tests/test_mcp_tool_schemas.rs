use superstate_mcp::server::{handle_json_rpc, JsonRpcRequest};
use superstate_mcp::tools::list_all_mcp_tools;

#[test]
fn test_all_cli_parity_tools_declared() {
    let tools = list_all_mcp_tools();
    let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

    let expected = [
        "superstate_handoff",
        "superstate_status",
        "superstate_cancel",
        "superstate_doctor",
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
        4,
        "Expected exactly 4 Gateway tools, found {}",
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

    // superstate_handoff schema
    let handoff = find_tool("superstate_handoff");
    let handoff_props = &handoff.input_schema.properties;
    assert_eq!(handoff_props["goal"]["type"], "string");
    assert_eq!(handoff_props["milestones"]["type"], "array");
    assert_eq!(handoff_props["verify_cmd"]["type"], "string");
    assert_eq!(handoff_props["driver"]["type"], "string");
    assert_eq!(handoff_props["detach"]["type"], "boolean");
    assert_eq!(handoff_props["files"]["type"], "array");

    // superstate_status schema
    let status = find_tool("superstate_status");
    let status_props = &status.input_schema.properties;
    assert_eq!(status_props["state"]["type"], "string");
    assert_eq!(status_props["json"]["type"], "boolean");

    // superstate_cancel schema
    let cancel = find_tool("superstate_cancel");
    let cancel_props = &cancel.input_schema.properties;
    assert_eq!(cancel_props["slug"]["type"], "string");
    assert_eq!(cancel_props["state"]["type"], "string");
    assert_eq!(cancel_props["all"]["type"], "boolean");
    assert_eq!(cancel_props["branch"]["type"], "string");

    // superstate_doctor schema
    let doctor = find_tool("superstate_doctor");
    let doctor_props = &doctor.input_schema.properties;
    assert_eq!(doctor_props["driver"]["type"], "string");
    assert_eq!(doctor_props["repair"]["type"], "boolean");
    assert_eq!(doctor_props["json"]["type"], "boolean");
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
    assert_eq!(tools.len(), 4);
    for t in tools {
        assert!(
            t.get("inputSchema").is_some(),
            "Tool missing standard MCP inputSchema: {:?}",
            t.get("name")
        );
    }
}
