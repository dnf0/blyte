use serde_json::json;
use std::path::PathBuf;
use superstate_core::ledger::read_ledger;
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{branch_scoped_state_path, load_state, save_state_atomic};
use superstate_mcp::server::{handle_json_rpc, handle_tool_call, JsonRpcRequest};
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

fn extract_text_content(val: &serde_json::Value) -> String {
    if let Some(s) = val.get("text").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(s) = val.get("output").and_then(|v| v.as_str()) {
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
fn test_state_ready_returns_unblocked_tasks() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1 Root".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        completed_tasks: vec![],
        pending_tasks: vec![
            TaskSpec {
                id: "task-2".to_string(),
                title: "Task 2 Depends on 1".to_string(),
                depends_on: vec!["task-1".to_string()],
                files_modify: vec![],
                interfaces_produces: vec![],
                verification_cmd: None,
            },
            TaskSpec {
                id: "task-3".to_string(),
                title: "Task 3 Independent".to_string(),
                depends_on: vec![],
                files_modify: vec![],
                interfaces_produces: vec![],
                verification_cmd: None,
            },
        ],
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Test json = true
    let args_json = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });
    let res_json = handle_tool_call("state_ready", &args_json).expect("state_ready should succeed");
    let ready_arr = res_json
        .get("ready")
        .and_then(|v| v.as_array())
        .expect("ready array");
    let titles: Vec<&str> = ready_arr.iter().filter_map(|v| v.as_str()).collect();

    assert!(titles.contains(&"task-1: Task 1 Root") || titles.contains(&"Task 1 Root"));
    assert!(
        titles.contains(&"task-3: Task 3 Independent") || titles.contains(&"Task 3 Independent")
    );
    assert!(!titles.iter().any(|t| t.contains("Task 2 Depends on 1")));

    // Test json = false
    let args_text = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": false
    });
    let res_text =
        handle_tool_call("state_ready", &args_text).expect("state_ready text should succeed");
    let text = extract_text_content(&res_text);
    assert!(text.contains("Unblocked tasks"), "got text: {}", text);
    assert!(text.contains("task-1"), "got text: {}", text);
    assert!(text.contains("task-3"), "got text: {}", text);
}

#[test]
fn test_state_sync_reconciles_plan_and_preserves_completed() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 2,
        completed_tasks: vec![TaskSpec {
            id: "Task 1".to_string(),
            title: "Task 1 Completed".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        active_task: Some(TaskSpec {
            id: "Task 2".to_string(),
            title: "Task 2 Dropped in New Plan".to_string(),
            depends_on: vec!["Task 1".to_string()],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        pending_tasks: vec![TaskSpec {
            id: "Task 3".to_string(),
            title: "Task 3 Dropped".to_string(),
            depends_on: vec!["Task 2".to_string()],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let plan_file = repo_root.join("revised_plan.md");
    let plan_content = r#"# Implementation Plan
### Task 1: Task 1 Completed
- **Depends on:** none

### Task 4: Task 4 Added
- **Depends on:** Task 1
"#;
    std::fs::write(&plan_file, plan_content).unwrap();

    let out_file = repo_root.join(".agent-state").join("reconciled.json");
    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "plan": plan_file.to_str().unwrap(),
        "out": out_file.to_str().unwrap()
    });

    let res = handle_tool_call("state_sync", &args).expect("state_sync should succeed");
    assert_eq!(res["status"], "success");
    assert_eq!(res["preserved_completed"], 1);

    // Verify out file contains reconciled state
    let reconciled = load_state(&out_file).expect("reconciled state should exist");
    assert_eq!(reconciled.completed_tasks.len(), 1);
    assert_eq!(reconciled.completed_tasks[0].id, "Task 1");
    assert_eq!(reconciled.active_task.as_ref().unwrap().id, "Task 4");
    assert_eq!(reconciled.plan_path, Some(plan_file));
}

#[test]
fn test_state_doctor_diagnoses_and_repairs() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    // Inconsistent state: task-1 is in both completed_tasks and pending_tasks
    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        completed_tasks: vec![TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1 Done".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        active_task: None,
        pending_tasks: vec![TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1 Done".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Diagnose without repair
    let args_diag = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "repair": false,
        "json": true
    });
    let res_diag =
        handle_tool_call("state_doctor", &args_diag).expect("state_doctor diagnose should succeed");
    assert_eq!(res_diag["is_healthy"], false);
    let issues = res_diag["issues"].as_array().expect("issues array");
    assert!(!issues.is_empty());

    // Run with repair: true
    let args_repair = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "repair": true,
        "json": true
    });
    let res_repair =
        handle_tool_call("state_doctor", &args_repair).expect("state_doctor repair should succeed");
    let repaired_actions = res_repair["repaired_actions"]
        .as_array()
        .expect("repaired_actions array");
    assert!(
        !repaired_actions.is_empty(),
        "should report repaired actions"
    );

    // Verify on-disk state is repaired
    let state_repaired = load_state(&state_file).expect("state should be readable");
    assert_eq!(state_repaired.completed_tasks.len(), 1);
    assert_eq!(state_repaired.pending_tasks.len(), 0);
}

#[test]
fn test_state_validate_valid_and_invalid() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let valid_state = json!({
        "status": "InProgress",
        "step": 1,
        "completed_tasks": [],
        "pending_tasks": [],
        "errors": [],
        "notes": [],
        "modified_files": []
    });
    std::fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    std::fs::write(
        &state_file,
        serde_json::to_string_pretty(&valid_state).unwrap(),
    )
    .unwrap();

    let args_valid = json!({
        "state": state_file.to_str().unwrap()
    });
    let res_valid =
        handle_tool_call("state_validate", &args_valid).expect("state_validate should succeed");
    assert_eq!(res_valid["valid"], true);
    let text = extract_text_content(&res_valid);
    assert!(text.contains("State is valid"));

    // Make state invalid by removing required fields and invalid status
    let invalid_state = json!({
        "status": "InvalidStatus123",
        "step": "not-a-number"
    });
    std::fs::write(
        &state_file,
        serde_json::to_string_pretty(&invalid_state).unwrap(),
    )
    .unwrap();

    let res_invalid = handle_tool_call("state_validate", &args_valid)
        .expect("state_validate should return result");
    assert_eq!(res_invalid["valid"], false);
    let errors = res_invalid["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty());
}

#[test]
fn test_state_apply_patch_and_note() {
    let (_dir, repo_root) = setup_test_repo();
    let state_file = repo_root.join(".agent-state").join("state.json");

    let initial_state = json!({
        "status": "in_progress",
        "step": 1,
        "completed_tasks": [],
        "pending_tasks": [],
        "errors": [],
        "notes": [],
        "modified_files": [],
        "to_be_deleted": "delete_me",
        "goal": "Initial goal"
    });
    std::fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    std::fs::write(
        &state_file,
        serde_json::to_string_pretty(&initial_state).unwrap(),
    )
    .unwrap();

    // Test error when neither patch nor note is provided
    let args_empty = json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let err = handle_tool_call("state_apply", &args_empty).unwrap_err();
    assert!(err.contains("Nothing to apply"));

    // Apply inline patch with null deletion and modify goal, plus note
    let patch = json!({
        "to_be_deleted": null,
        "goal": "Updated goal"
    });
    let out_file = repo_root.join(".agent-state").join("patched.json");
    let args_apply = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "patch": patch,
        "note": "Hypothesis: patch was applied cleanly",
        "out": out_file.to_str().unwrap()
    });

    let res_apply =
        handle_tool_call("state_apply", &args_apply).expect("state_apply should succeed");
    assert_eq!(res_apply["status"], "success");

    // Check patched state
    let patched_raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out_file).unwrap()).unwrap();
    assert_eq!(patched_raw.get("to_be_deleted"), None);
    assert_eq!(patched_raw["goal"], "Updated goal");

    // Check ledger note
    let ledger = read_ledger(&state_file);
    assert!(ledger
        .iter()
        .any(|entry| entry.note.contains("patch was applied cleanly")));
}

#[test]
fn test_state_list_discovers_states() {
    let (_dir, repo_root) = setup_test_repo();

    // 1. Primary state
    let state_file = repo_root.join(".agent-state").join("state.json");
    let main_state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        base_branch: Some("main".to_string()),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1 Primary".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }),
        ..Default::default()
    };
    save_state_atomic(&state_file, &main_state).unwrap();

    // 2. Branch state
    let branch_state_file = branch_scoped_state_path(&repo_root, "feat/user-auth");
    let branch_state = ExecutionState {
        status: ExecutionStatus::Completed,
        step: 2,
        base_branch: Some("main".to_string()),
        active_task: None,
        completed_tasks: vec![TaskSpec {
            id: "task-auth".to_string(),
            title: "Auth Implementation".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        ..Default::default()
    };
    save_state_atomic(&branch_state_file, &branch_state).unwrap();

    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });

    let res = handle_tool_call("state_list", &args).expect("state_list should succeed");
    let states = res
        .get("states")
        .or_else(|| res.get("branches"))
        .and_then(|v| v.as_array())
        .expect("states array");
    assert!(states.len() >= 2);

    let branches: Vec<&str> = states
        .iter()
        .filter_map(|s| s.get("branch").and_then(|b| b.as_str()))
        .collect();

    assert!(branches
        .iter()
        .any(|b| b.contains("main") || b.contains("default")));
    assert!(branches
        .iter()
        .any(|b| b.contains("feat-user-auth") || b.contains("feat/user-auth")));

    // JSON-RPC tools/call dispatch test
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(42)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "state_list",
            "arguments": {
                "repo_root": repo_root.to_str().unwrap(),
                "json": false
            }
        })),
    };
    let resp = handle_json_rpc(req);
    assert!(resp.error.is_none());
    let rpc_res = resp.result.expect("result should exist");
    let text = extract_text_content(&rpc_res);
    assert!(
        text.contains("Branches") || text.contains("Active") || text.contains("main"),
        "got text: {}",
        text
    );
}
