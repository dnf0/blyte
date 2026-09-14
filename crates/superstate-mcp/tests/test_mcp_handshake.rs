use superstate_mcp::tools::list_all_mcp_tools;

#[test]
fn test_mcp_tools_list_contains_all_capabilities() {
    let tools = list_all_mcp_tools();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names.len(), 4, "Expected exactly 4 Gateway tools");
    assert!(names.contains(&"superstate_handoff"));
    assert!(names.contains(&"superstate_status"));
    assert!(names.contains(&"superstate_cancel"));
    assert!(names.contains(&"superstate_doctor"));
    assert!(!names.contains(&"state_init"));
    assert!(!names.contains(&"state_next"));
    assert!(!names.contains(&"state_prompt"));
    assert!(!names.contains(&"state_note"));
    assert!(!names.contains(&"superstate_review"));
}
