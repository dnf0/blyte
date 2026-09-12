use serde_json::json;
use std::path::PathBuf;
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::save_state_atomic;
use superstate_mcp::server::handle_tool_call;
use tempfile::tempdir;

fn setup_test_repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();

    let _ = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test Agent"])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@agent.local"])
        .current_dir(&repo_root)
        .output();

    let readme = repo_root.join("README.md");
    std::fs::write(&readme, "# Test Repo\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(&repo_root)
        .output();

    (temp, repo_root)
}

fn extract_prompt_text(val: &serde_json::Value) -> String {
    if let Some(s) = val.get("turnkey_prompt").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(s) = val.get("prompt").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(arr) = val.get("content").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                return s.to_string();
            }
        }
    }
    val.to_string()
}

#[test]
fn test_state_next_ignore_untracked_glob() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        no_worktree: true,
        base_branch: Some("main".to_string()),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1 Clean Verification".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: Some("echo 'ok'".to_string()),
        }),
        completed_tasks: vec![],
        pending_tasks: vec![],
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Create untracked file build.log
    let untracked = repo_root.join("build.log");
    std::fs::write(&untracked, "build output").unwrap();

    // Without ignore_untracked_glob, verify_git_clean: true must FAIL
    let args_fail = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_git_clean": true
    });
    let err = handle_tool_call("state_next", &args_fail).unwrap_err();
    assert!(
        err.contains("Working tree is not clean") || err.contains("git_clean"),
        "expected git clean failure, got: {}",
        err
    );

    // With ignore_untracked_glob: "build.log", verify_git_clean: true must PASS
    let args_pass = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_git_clean": true,
        "ignore_untracked_glob": "build.log"
    });
    let res = handle_tool_call("state_next", &args_pass)
        .expect("state_next should pass with ignored untracked file");
    assert_eq!(res["status"], "success");
    assert_eq!(res["step"], 2);
}

#[test]
fn test_state_prompt_targeting_specific_task() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        no_worktree: true,
        base_branch: Some("main".to_string()),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "First Active Task".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        completed_tasks: vec![],
        pending_tasks: vec![TaskSpec {
            id: "task-2".to_string(),
            title: "Second Pending Task".to_string(),
            depends_on: vec!["task-1".to_string()],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Prompt for task-2 specifically
    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "task": "task-2"
    });
    let res =
        handle_tool_call("state_prompt", &args).expect("state_prompt for task-2 should succeed");
    let prompt = extract_prompt_text(&res);
    assert!(
        prompt.contains("task-2: Second Pending Task"),
        "got: {}",
        prompt
    );
    assert!(!prompt.contains("task-1: First Active Task"));

    // Prompt for non-existent task returns error
    let args_unknown = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "task": "task-999"
    });
    let err = handle_tool_call("state_prompt", &args_unknown).unwrap_err();
    assert!(err.contains("No task matching 'task-999'"), "got: {}", err);
}

#[test]
fn test_state_prompt_with_graph_parameter() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        no_worktree: true,
        base_branch: Some("main".to_string()),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Architecture Analysis".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        completed_tasks: vec![],
        pending_tasks: vec![],
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Create GRAPH_REPORT.md
    let graph_report = repo_root.join("GRAPH_REPORT.md");
    std::fs::write(
        &graph_report,
        "# Knowledge Graph Report\nArchitectural Topology: Cluster Alpha and Beta\n",
    )
    .unwrap();

    // Call with with_graph: true
    let args_with_graph = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "with_graph": true
    });
    let res_with = handle_tool_call("state_prompt", &args_with_graph)
        .expect("state_prompt with graph should succeed");
    let prompt_with = extract_prompt_text(&res_with);
    assert!(
        prompt_with.contains("Architectural Topology: Cluster Alpha and Beta"),
        "prompt should contain graph report content, got: {}",
        prompt_with
    );

    // Call with with_graph: false (or omitted)
    let args_without = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "with_graph": false
    });
    let res_without = handle_tool_call("state_prompt", &args_without)
        .expect("state_prompt without graph should succeed");
    let prompt_without = extract_prompt_text(&res_without);
    assert!(
        !prompt_without.contains("Architectural Topology: Cluster Alpha and Beta"),
        "prompt should not contain graph report when with_graph is false"
    );
}
