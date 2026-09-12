use std::path::PathBuf;
use superstate_core::models::{ExecutionState, TaskSpec};
use superstate_core::prompt::render_turnkey_prompt;

#[test]
fn test_render_turnkey_prompt_includes_global_goal() {
    let state = ExecutionState {
        goal: Some(
            "Implement a high-performance streaming parser for JSON2 with byte-exact output."
                .to_string(),
        ),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Add streaming buffer".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }),
        ..Default::default()
    };

    let state_path = PathBuf::from("/tmp/test-state/state.json");
    let rendered = render_turnkey_prompt(&state, &state_path, None, true, false).unwrap();

    assert!(rendered.contains("## 🎯 Global Task Objective & Requirements"));
    assert!(rendered.contains(
        "Implement a high-performance streaming parser for JSON2 with byte-exact output."
    ));
    assert!(rendered.contains("Add streaming buffer"));
}

#[test]
fn test_render_turnkey_prompt_without_goal() {
    let state = ExecutionState {
        goal: None,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Add streaming buffer".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }),
        ..Default::default()
    };

    let state_path = PathBuf::from("/tmp/test-state/state.json");
    let rendered = render_turnkey_prompt(&state, &state_path, None, true, false).unwrap();

    assert!(!rendered.contains("## 🎯 Global Task Objective & Requirements"));
    assert!(rendered.contains("Add streaming buffer"));
}

#[test]
fn test_render_turnkey_prompt_with_empty_or_whitespace_goal() {
    let state = ExecutionState {
        goal: Some("   \n\t  ".to_string()),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Add streaming buffer".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }),
        ..Default::default()
    };

    let state_path = PathBuf::from("/tmp/test-state/state.json");
    let rendered = render_turnkey_prompt(&state, &state_path, None, true, false).unwrap();

    assert!(!rendered.contains("## 🎯 Global Task Objective & Requirements"));
    assert!(rendered.contains("Add streaming buffer"));
}
