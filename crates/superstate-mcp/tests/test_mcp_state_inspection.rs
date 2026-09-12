use serde_json::json;
use std::path::PathBuf;
use superstate_core::ledger::append_ledger_note;
use superstate_core::models::{ExecutionState, ExecutionStatus, LedgerEntry, TaskSpec};
use superstate_core::state_machine::save_state_atomic;
use superstate_mcp::server::{handle_json_rpc, handle_tool_call, JsonRpcRequest};
use tempfile::tempdir;

fn setup_test_repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();

    // Initialize git repository with an initial commit
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

    let initial_file = repo_root.join("README.md");
    std::fs::write(&initial_file, "# Test Repo\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(&repo_root)
        .output();

    // Create state file
    let state_file = repo_root.join(".agent-state").join("state.json");
    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 2,
        base_branch: Some("main".to_string()),
        active_task: Some(TaskSpec {
            id: "task-2".to_string(),
            title: "Implement Inspection Handlers".to_string(),
            depends_on: vec!["task-1".to_string()],
            files_modify: vec![PathBuf::from("crates/superstate-mcp/src/server.rs")],
            interfaces_produces: vec![],
            verification_cmd: Some("echo 'task-2 verify'".to_string()),
        }),
        completed_tasks: vec![TaskSpec {
            id: "task-1".to_string(),
            title: "Initial Setup".to_string(),
            depends_on: vec![],
            files_modify: vec![PathBuf::from("README.md")],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }],
        pending_tasks: vec![TaskSpec {
            id: "task-3".to_string(),
            title: "Safety Handlers".to_string(),
            depends_on: vec!["task-2".to_string()],
            files_modify: vec![PathBuf::from("crates/superstate-mcp/src/server.rs")],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }],
        ..Default::default()
    };

    save_state_atomic(&state_file, &state).expect("failed to save state");
    append_ledger_note(&state_file, LedgerEntry::new("agent", "Invariant verified"))
        .expect("failed to append note");

    (temp, repo_root)
}

fn extract_text_content(val: &serde_json::Value) -> String {
    if let Some(s) = val.get("text").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(s) = val.get("output").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(s) = val.get("markdown").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(s) = val.get("briefing").and_then(|v| v.as_str()) {
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
fn test_state_status_text() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("state_status", &args).expect("state_status should succeed");
    let text = extract_text_content(&res);

    assert!(text.contains("State Status: InProgress"), "got: {}", text);
    assert!(text.contains("Step: 2"), "got: {}", text);
    assert!(
        text.contains("Active Task: task-2: Implement Inspection Handlers"),
        "got: {}",
        text
    );
    assert!(text.contains("Completed Tasks: 1"), "got: {}", text);
    assert!(text.contains("Ledger Entries: 1"), "got: {}", text);
    assert!(text.contains("✓ task-1: Initial Setup"), "got: {}", text);
    assert!(
        text.contains("Recent Notes (Epistemic Ledger):"),
        "got: {}",
        text
    );
    assert!(
        text.contains("• [agent] Invariant verified"),
        "got: {}",
        text
    );
}

#[test]
fn test_state_status_json() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });

    let res = handle_tool_call("state_status", &args).expect("state_status should succeed");
    let step = res
        .get("step")
        .or_else(|| res.get("state").and_then(|s| s.get("step")))
        .and_then(|v| v.as_u64())
        .expect("step should be present");
    assert_eq!(step, 2);

    let active_task_id = res
        .get("active_task")
        .or_else(|| res.get("state").and_then(|s| s.get("active_task")))
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_str())
        .expect("active_task.id should be present");
    assert_eq!(active_task_id, "task-2");
}

#[test]
fn test_state_show_markdown() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("state_show", &args).expect("state_show should succeed");
    let md = extract_text_content(&res);

    assert!(md.contains("# Superstate Execution Status"), "got: {}", md);
    assert!(
        md.contains("- **Status:** InProgress (Step 2)"),
        "got: {}",
        md
    );
    assert!(md.contains("- **Base Branch:** `main`"), "got: {}", md);
    assert!(
        md.contains("- **Active Task:** task-2: Implement Inspection Handlers"),
        "got: {}",
        md
    );
    assert!(
        md.contains("- **Verification Command:** `echo 'task-2 verify'`"),
        "got: {}",
        md
    );
}

#[test]
fn test_state_show_json() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });

    let res = handle_tool_call("state_show", &args).expect("state_show should succeed");
    let step = res
        .get("step")
        .or_else(|| res.get("state").and_then(|s| s.get("step")))
        .and_then(|v| v.as_u64())
        .expect("step should be present");
    assert_eq!(step, 2);

    let active_task_id = res
        .get("active_task")
        .or_else(|| res.get("state").and_then(|s| s.get("active_task")))
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_str())
        .expect("active_task.id should be present");
    assert_eq!(active_task_id, "task-2");
}

#[test]
fn test_state_resume() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("state_resume", &args).expect("state_resume should succeed");
    let briefing = extract_text_content(&res);

    assert!(
        briefing.contains("# Agent Execution Resume Briefing"),
        "got: {}",
        briefing
    );
    assert!(
        briefing.contains("- **Current Status:** `InProgress` (Step 2)"),
        "got: {}",
        briefing
    );
    assert!(
        briefing.contains("- **Active Task:** task-2: Implement Inspection Handlers"),
        "got: {}",
        briefing
    );
    assert!(
        briefing.contains("## Recorded Findings"),
        "got: {}",
        briefing
    );
    assert!(
        briefing.contains("- [agent] Invariant verified"),
        "got: {}",
        briefing
    );
    assert!(
        briefing.contains("## Recent Git Commits"),
        "got: {}",
        briefing
    );
    assert!(
        briefing.contains("chore: initial commit"),
        "got: {}",
        briefing
    );
}

#[test]
fn test_state_metrics_text() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("state_metrics", &args).expect("state_metrics should succeed");
    let text = extract_text_content(&res);

    assert!(text.contains("Step:            2"), "got: {}", text);
    assert!(
        text.contains("Status:          InProgress"),
        "got: {}",
        text
    );
    assert!(
        text.contains("Tasks:           1/3 complete (33.3%)"),
        "got: {}",
        text
    );
    assert!(text.contains("Remaining:       2"), "got: {}", text);
    assert!(text.contains("Notes recorded:  1"), "got: {}", text);
    assert!(text.contains("Errors recorded: 0"), "got: {}", text);
}

#[test]
fn test_state_metrics_json() {
    let (_dir, repo_root) = setup_test_repo();
    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });

    let res = handle_tool_call("state_metrics", &args).expect("state_metrics should succeed");
    let step = res
        .get("step")
        .or_else(|| res.get("metrics").and_then(|m| m.get("step")))
        .and_then(|v| v.as_u64())
        .expect("step should be present");
    assert_eq!(step, 2);

    let completed = res
        .get("tasks_completed")
        .or_else(|| res.get("metrics").and_then(|m| m.get("tasks_completed")))
        .and_then(|v| v.as_u64())
        .expect("tasks_completed should be present");
    assert_eq!(completed, 1);

    let total = res
        .get("tasks_total")
        .or_else(|| res.get("metrics").and_then(|m| m.get("tasks_total")))
        .and_then(|v| v.as_u64())
        .expect("tasks_total should be present");
    assert_eq!(total, 3);
}

#[test]
fn test_state_inspection_json_rpc() {
    let (_dir, repo_root) = setup_test_repo();

    let tools = [
        "state_status",
        "state_show",
        "state_resume",
        "state_metrics",
    ];
    for tool in tools {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": tool,
                "arguments": {
                    "repo_root": repo_root.to_str().unwrap()
                }
            })),
        };

        let resp = handle_json_rpc(req);
        assert!(
            resp.error.is_none(),
            "tool {} returned error: {:?}",
            tool,
            resp.error
        );
        assert!(resp.result.is_some(), "tool {} must have result", tool);
    }
}
