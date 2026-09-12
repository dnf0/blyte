use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{load_state, save_state_atomic};
use superstate_mcp::server::{handle_json_rpc, handle_tool_call, JsonRpcRequest};
use tempfile::tempdir;

fn setup_git_repo_with_state() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();

    // Initialize git repository with initial commit
    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test Agent"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@agent.local"])
        .current_dir(&repo_root)
        .output()
        .unwrap();

    let initial_file = repo_root.join("README.md");
    std::fs::write(&initial_file, "# Initial Content\n").unwrap();
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(&repo_root)
        .output()
        .unwrap();

    // Create state file
    let state_file = repo_root.join(".agent-state").join("state.json");
    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        base_branch: Some("main".to_string()),
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task 1 Safety".to_string(),
            depends_on: vec![],
            files_modify: vec![PathBuf::from("README.md")],
            interfaces_produces: vec![],
            verification_cmd: Some("echo 'task-1 passed'".to_string()),
        }),
        pending_tasks: vec![TaskSpec {
            id: "task-2".to_string(),
            title: "Task 2 Next Step".to_string(),
            depends_on: vec!["task-1".to_string()],
            files_modify: vec![PathBuf::from("README.md")],
            interfaces_produces: vec![],
            verification_cmd: Some("echo 'task-2 passed'".to_string()),
        }],
        completed_tasks: vec![],
        ..Default::default()
    };

    save_state_atomic(&state_file, &state).expect("failed to save initial state");

    (temp, repo_root, state_file)
}

#[test]
fn test_state_checkpoint_creates_git_tag() {
    let (_dir, repo_root, _state_file) = setup_git_repo_with_state();

    let args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("state_checkpoint", &args)
        .expect("state_checkpoint tool call should succeed");

    assert_eq!(res["checkpoint"], "agent-checkpoint/step-1");
    assert_eq!(res["step"], 1);

    let content_text = res["content"][0]["text"].as_str().unwrap();
    assert!(
        content_text.contains("Checkpoint created: agent-checkpoint/step-1"),
        "got text: {}",
        content_text
    );

    // Verify git tag exists in repository
    let git_tag_output = Command::new("git")
        .args(["tag", "-l", "agent-checkpoint/step-1"])
        .current_dir(&repo_root)
        .output()
        .expect("git tag command failed");

    let stdout = String::from_utf8_lossy(&git_tag_output.stdout);
    assert_eq!(stdout.trim(), "agent-checkpoint/step-1");
}

#[test]
fn test_state_rollback_restores_modified_file_and_preserves_state() {
    let (_dir, repo_root, state_file) = setup_git_repo_with_state();

    // 1. Create a checkpoint at step 1
    let cp_args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    handle_tool_call("state_checkpoint", &cp_args).expect("state_checkpoint failed");

    // 2. Modify README.md and add an untracked file
    let readme_path = repo_root.join("README.md");
    std::fs::write(
        &readme_path,
        "# Modified Content that Should Be Rolled Back\n",
    )
    .unwrap();

    let untracked_path = repo_root.join("debris.txt");
    std::fs::write(&untracked_path, "temporary debris\n").unwrap();
    assert!(untracked_path.exists());

    // 3. Update state.json with a note so we can verify it survived rollback
    let mut state = load_state(&state_file).unwrap();
    state.notes.push(superstate_core::models::LedgerEntry::new(
        "agent",
        "Note before rollback",
    ));
    save_state_atomic(&state_file, &state).unwrap();

    // 4. Call state_rollback
    let rb_args = json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let res = handle_tool_call("state_rollback", &rb_args)
        .expect("state_rollback tool call should succeed");

    assert_eq!(res["target"], "agent-checkpoint/step-1");
    assert_eq!(res["step"], 1);

    let content_text = res["content"][0]["text"].as_str().unwrap();
    assert!(
        content_text.contains("Working tree cleanly rolled back to agent-checkpoint/step-1"),
        "got: {}",
        content_text
    );

    // 5. Verify README.md restored to original content
    let restored_content = std::fs::read_to_string(&readme_path).unwrap();
    assert_eq!(restored_content, "# Initial Content\n");

    // 6. Verify untracked debris file was deleted
    assert!(
        !untracked_path.exists(),
        "debris.txt should have been cleaned up"
    );

    // 7. Verify .agent-state/state.json was preserved and retains the note
    let reloaded_state = load_state(&state_file).expect("state.json should survive rollback");
    assert_eq!(reloaded_state.step, 1);
    assert_eq!(reloaded_state.notes.len(), 1);
    assert_eq!(reloaded_state.notes[0].note, "Note before rollback");
}

#[test]
fn test_state_exec_passes_and_advances_step() {
    let (_dir, repo_root, state_file) = setup_git_repo_with_state();

    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_cmd": "echo 'verification test passed'",
        "verify_git_clean": true,
        "checkpoint": true,
        "advance": true
    });

    let res = handle_tool_call("state_exec", &args).expect("state_exec should succeed");

    assert_eq!(res["passed"], true);
    assert_eq!(res["advanced"], true);
    assert_eq!(res["rolled_back"], false);
    assert_eq!(res["step"], 1);
    assert!(res["output"]
        .as_str()
        .unwrap()
        .contains("verification test passed"));

    // Verify git tag was created due to checkpoint: true
    let git_tag_output = Command::new("git")
        .args(["tag", "-l", "agent-checkpoint/step-1"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&git_tag_output.stdout).trim(),
        "agent-checkpoint/step-1"
    );

    // Verify state advanced to step 2 on disk
    let updated_state = load_state(&state_file).unwrap();
    assert_eq!(updated_state.step, 2);
    assert_eq!(updated_state.completed_tasks.len(), 1);
    assert_eq!(updated_state.completed_tasks[0].id, "task-1");
    assert_eq!(
        updated_state.active_task.as_ref().map(|t| t.id.as_str()),
        Some("task-2")
    );
}

#[test]
fn test_state_exec_fails_records_error_and_rolls_back() {
    let (_dir, repo_root, state_file) = setup_git_repo_with_state();

    let readme_path = repo_root.join("README.md");

    let fail_cmd = "echo 'dirty modification' >> README.md && echo 'failure simulation' > failed_debris.txt && exit 42";

    let args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_cmd": fail_cmd,
        "checkpoint": true,
        "rollback_on_fail": true,
        "advance": true
    });

    let res = handle_tool_call("state_exec", &args)
        .expect("state_exec returns result object on gate failure");

    assert_eq!(res["passed"], false);
    assert_eq!(res["advanced"], false);
    assert_eq!(res["rolled_back"], true);
    assert_eq!(res["step"], 1);

    // Verify state did NOT advance
    let state_after = load_state(&state_file).unwrap();
    assert_eq!(state_after.step, 1);

    // Verify error was recorded in state.errors
    assert!(
        !state_after.errors.is_empty(),
        "state.errors should contain failure record"
    );
    assert!(
        state_after.errors[0].contains("verification failed"),
        "error record: {}",
        state_after.errors[0]
    );

    // Verify README.md was rolled back to initial commit state
    let readme_content = std::fs::read_to_string(&readme_path).unwrap();
    assert_eq!(readme_content, "# Initial Content\n");

    // Verify debris file was removed by rollback
    let debris_path = repo_root.join("failed_debris.txt");
    assert!(
        !debris_path.exists(),
        "failed_debris.txt should have been removed by rollback"
    );
}

#[test]
fn test_state_exec_json_rpc_dispatch() {
    let (_dir, repo_root, _state_file) = setup_git_repo_with_state();

    let rpc_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(101)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "state_exec",
            "arguments": {
                "repo_root": repo_root.to_str().unwrap(),
                "verify_cmd": "echo 'jsonrpc test'",
                "advance": false
            }
        })),
    };

    let rpc_res = handle_json_rpc(rpc_req);
    assert!(rpc_res.error.is_none());
    let result = rpc_res.result.unwrap();
    assert_eq!(result["passed"], true);
    assert_eq!(result["advanced"], false);
    assert_eq!(result["rolled_back"], false);
    assert_eq!(result["step"], 1);
}
