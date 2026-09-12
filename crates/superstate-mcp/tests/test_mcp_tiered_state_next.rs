use serde_json::json;
use std::fs;
use std::process::Command;
use superstate_mcp::server::handle_tool_call;
use tempfile::tempdir;

fn setup_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempdir().unwrap();
    let root = temp.path().to_path_buf();

    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test Agent"])
        .current_dir(&root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@agent.local"])
        .current_dir(&root)
        .output()
        .unwrap();

    fs::write(root.join("README.md"), "# Test Repo\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(&root)
        .output()
        .unwrap();

    (temp, root)
}

#[test]
fn test_state_next_milestone_and_final_stages() {
    let (_temp, root) = setup_test_repo();

    // Initialize 2-milestone execution state
    let init_args = json!({
        "repo_root": root.to_str().unwrap(),
        "milestones": [
            {"id": "task-1", "title": "M1", "verify_cmd": "echo m1"},
            {"id": "task-2", "title": "M2", "verify_cmd": "echo m2"}
        ],
        "no_worktree": true
    });
    handle_tool_call("superstate_handoff", &init_args).expect("superstate_handoff should succeed");

    // 1. Advance task-1 with milestone stage (fast verification)
    let next_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_cmd": "true",
        "verify_stage": "milestone"
    });
    let res = handle_tool_call("state_next", &next_args)
        .expect("state_next with verify_stage: milestone should succeed");
    assert_eq!(res["status"], "success");
    assert_eq!(res["stage"], "milestone");
    assert_eq!(res["active_task"]["id"], "task-2");

    // 2. Advance task-2 with final stage and failing full suite command
    let final_fail_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_cmd": "false",
        "verify_stage": "final"
    });
    let final_fail_res = handle_tool_call("state_next", &final_fail_args);
    assert!(
        final_fail_res.is_err(),
        "Final stage must enforce full regression check and block advancement on failure"
    );

    // Verify task-2 is still active after failed final stage gate
    let show_args = json!({
        "repo_root": root.to_str().unwrap(),
        "json": true
    });
    let show_res = handle_tool_call("state_show", &show_args).expect("state_show should succeed");
    assert_eq!(
        show_res["active_task"]["id"], "task-2",
        "active_task must remain task-2 after failed verification"
    );

    // 3. Advance task-2 with final stage and passing command
    let final_pass_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_cmd": "true",
        "verify_stage": "final"
    });
    let final_pass_res = handle_tool_call("state_next", &final_pass_args)
        .expect("state_next with passing final stage must succeed");
    assert_eq!(final_pass_res["status"], "success");
    assert_eq!(final_pass_res["stage"], "final");
    assert!(
        final_pass_res["active_task"].is_null(),
        "active_task should be null after completing final task"
    );

    // Verify state transitioned to completed
    let final_show_res =
        handle_tool_call("state_show", &show_args).expect("state_show should succeed");
    assert_eq!(final_show_res["status"], "completed");
}

#[test]
fn test_state_next_defaults_to_milestone_stage() {
    let (_temp, root) = setup_test_repo();

    let init_args = json!({
        "repo_root": root.to_str().unwrap(),
        "milestones": [
            {"id": "task-1", "title": "M1", "verify_cmd": "true"}
        ],
        "no_worktree": true
    });
    handle_tool_call("superstate_handoff", &init_args).expect("superstate_handoff should succeed");

    // Advance task-1 without specifying verify_stage
    let next_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_cmd": "true"
    });
    let res = handle_tool_call("state_next", &next_args)
        .expect("state_next without verify_stage should succeed");
    assert_eq!(res["status"], "success");
    assert_eq!(res["stage"], "milestone");
}

#[test]
fn test_state_next_rejects_invalid_stage() {
    let (_temp, root) = setup_test_repo();

    let init_args = json!({
        "repo_root": root.to_str().unwrap(),
        "milestones": [
            {"id": "task-1", "title": "M1", "verify_cmd": "true"}
        ],
        "no_worktree": true
    });
    handle_tool_call("superstate_handoff", &init_args).expect("superstate_handoff should succeed");

    let invalid_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_stage": "invalid_stage"
    });
    let err = handle_tool_call("state_next", &invalid_args).unwrap_err();
    assert!(
        err.contains("Invalid verify_stage"),
        "Expected error message mentioning Invalid verify_stage, got: {}",
        err
    );
}
