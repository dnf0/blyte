use serde_json::json;
use std::path::PathBuf;
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

#[test]
fn test_worktree_list_text_and_json_modes() {
    let (_dir, repo_root) = setup_test_repo();

    // 1. Create two worktrees via worktree_create
    let create1 = handle_tool_call(
        "worktree_create",
        &json!({
            "repo_root": repo_root.to_str().unwrap(),
            "branch": "wt/task-1"
        }),
    )
    .expect("worktree_create wt/task-1 should succeed");
    assert_eq!(create1["status"], "success");

    let create2 = handle_tool_call(
        "worktree_create",
        &json!({
            "repo_root": repo_root.to_str().unwrap(),
            "branch": "wt/task-2"
        }),
    )
    .expect("worktree_create wt/task-2 should succeed");
    assert_eq!(create2["status"], "success");

    // 2. Call worktree_list with json: true
    let list_json = handle_tool_call(
        "worktree_list",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "json": true
        }),
    )
    .expect("worktree_list with json: true should succeed");

    assert_eq!(list_json["status"], "success");
    let worktrees = list_json["worktrees"]
        .as_array()
        .expect("worktrees should be an array");
    assert_eq!(
        worktrees.len(),
        3,
        "expected 3 worktrees (main + 2 created)"
    );

    let branches: Vec<&str> = worktrees
        .iter()
        .filter_map(|w| w["branch"].as_str())
        .collect();
    assert!(branches.contains(&"main"), "should contain main branch");
    assert!(branches.contains(&"wt/task-1"), "should contain wt/task-1");
    assert!(branches.contains(&"wt/task-2"), "should contain wt/task-2");

    for wt in worktrees {
        assert!(wt["path"].as_str().is_some(), "path should be string");
        assert!(
            wt["commit_sha"].as_str().is_some() || wt["head"].as_str().is_some(),
            "commit_sha/head should be present"
        );
    }

    // 3. Call worktree_list with json: false (text mode)
    let list_text = handle_tool_call(
        "worktree_list",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "json": false
        }),
    )
    .expect("worktree_list with json: false should succeed");

    assert_eq!(list_text["status"], "success");
    let text_content = list_text["content"][0]["text"]
        .as_str()
        .or_else(|| list_text["text"].as_str())
        .expect("text content should be present");

    assert!(
        text_content.contains("BRANCH")
            && text_content.contains("HEAD")
            && text_content.contains("PATH"),
        "text table should have table headers"
    );
    assert!(
        text_content.contains("wt/task-1"),
        "text output should list wt/task-1"
    );
    assert!(
        text_content.contains("wt/task-2"),
        "text output should list wt/task-2"
    );
    assert!(
        text_content.contains("main [main]"),
        "text output should indicate main worktree"
    );
}

#[test]
fn test_worktree_remove_success_and_cleanup() {
    let (_dir, repo_root) = setup_test_repo();

    // Create worktree
    let create_res = handle_tool_call(
        "worktree_create",
        &json!({
            "repo_root": repo_root.to_str().unwrap(),
            "branch": "wt/to-remove"
        }),
    )
    .expect("worktree_create should succeed");

    let wt_path = PathBuf::from(create_res["worktree_path"].as_str().unwrap());
    assert!(wt_path.exists(), "worktree path must exist after creation");

    // Verify it appears in list
    let list_before = handle_tool_call(
        "worktree_list",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "json": true
        }),
    )
    .expect("worktree_list should succeed");
    let branches_before: Vec<&str> = list_before["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["branch"].as_str())
        .collect();
    assert!(branches_before.contains(&"wt/to-remove"));

    // Remove worktree
    let remove_res = handle_tool_call(
        "worktree_remove",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "branch": "wt/to-remove",
            "force": true
        }),
    )
    .expect("worktree_remove should succeed");

    assert_eq!(remove_res["branch"], "wt/to-remove");
    assert_eq!(remove_res["removed"], true);
    let msg = remove_res["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("Removed worktree for branch 'wt/to-remove'"),
        "content text should confirm removal, got: {}",
        msg
    );

    // Verify directory is deleted from disk
    assert!(
        !wt_path.exists(),
        "worktree directory should be deleted from disk"
    );

    // Verify it no longer appears in worktree_list
    let list_after = handle_tool_call(
        "worktree_list",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "json": true
        }),
    )
    .expect("worktree_list should succeed");
    let branches_after: Vec<&str> = list_after["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["branch"].as_str())
        .collect();
    assert!(
        !branches_after.contains(&"wt/to-remove"),
        "removed worktree should not appear in list"
    );
}

#[test]
fn test_worktree_remove_errors_and_guards() {
    let (_dir, repo_root) = setup_test_repo();

    // 1. Guard against removing primary worktree
    let err_main = handle_tool_call(
        "worktree_remove",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "branch": "main"
        }),
    )
    .unwrap_err();
    assert!(
        err_main.contains("Cannot remove primary worktree"),
        "expected error preventing removal of primary worktree, got: {}",
        err_main
    );

    // 2. Error when worktree does not exist
    let err_notfound = handle_tool_call(
        "worktree_remove",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "branch": "wt/does-not-exist"
        }),
    )
    .unwrap_err();
    assert!(
        err_notfound.contains("not found"),
        "expected not found error, got: {}",
        err_notfound
    );
}

#[test]
fn test_worktree_clean_orphaned_directories_and_branches() {
    let (_dir, repo_root) = setup_test_repo();

    // 1. Create an orphaned directory in .wt/
    let wt_dir = repo_root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    let orphan_dir = wt_dir.join("orphaned-task");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(orphan_dir.join("dummy.txt"), "leftover").unwrap();
    assert!(orphan_dir.exists());

    // 2. Create an orphaned branch wt/orphaned-branch in git (no worktree checked out)
    let branch_out = std::process::Command::new("git")
        .args(["branch", "wt/orphaned-branch"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    assert!(branch_out.status.success());

    // Verify branch exists before clean
    let branch_list_before = std::process::Command::new("git")
        .args(["branch", "--list", "wt/orphaned-branch"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    let before_str = String::from_utf8_lossy(&branch_list_before.stdout);
    assert!(before_str.contains("wt/orphaned-branch"));

    // 3. Call worktree_clean
    let clean_res = handle_tool_call(
        "worktree_clean",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "force": true
        }),
    )
    .expect("worktree_clean should succeed");

    assert!(clean_res["cleaned"].as_i64().unwrap() >= 2);
    let cleaned_branches: Vec<&str> = clean_res["branches"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        cleaned_branches.contains(&"orphaned-task")
            || cleaned_branches.contains(&"wt/orphaned-task"),
        "cleaned_branches should include orphaned-task"
    );
    assert!(
        cleaned_branches.contains(&"wt/orphaned-branch"),
        "cleaned_branches should include wt/orphaned-branch"
    );

    let content_text = clean_res["content"][0]["text"].as_str().unwrap();
    assert!(
        content_text.contains("Cleaned"),
        "content text should report cleaning: {}",
        content_text
    );

    // Verify orphan directory was deleted from disk
    assert!(!orphan_dir.exists(), "orphaned directory should be deleted");

    // Verify orphaned branch was deleted from git
    let branch_list_after = std::process::Command::new("git")
        .args(["branch", "--list", "wt/orphaned-branch"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    let after_str = String::from_utf8_lossy(&branch_list_after.stdout);
    assert!(
        !after_str.contains("wt/orphaned-branch"),
        "orphaned branch should be deleted from git"
    );

    // 4. Calling clean again when clean tree should report 0
    let clean_again = handle_tool_call(
        "worktree_clean",
        &json!({
            "repo": repo_root.to_str().unwrap(),
            "force": true
        }),
    )
    .expect("worktree_clean again should succeed");
    assert_eq!(clean_again["cleaned"], 0);
    assert_eq!(clean_again["branches"].as_array().unwrap().len(), 0);
}

#[test]
fn test_json_rpc_dispatch_all_three_tools() {
    let (_dir, repo_root) = setup_test_repo();

    // 1. worktree_list RPC
    let list_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "worktree_list",
            "arguments": {
                "repo": repo_root.to_str().unwrap(),
                "json": true
            }
        })),
    };
    let list_resp = handle_json_rpc(list_req);
    assert!(list_resp.error.is_none());
    let list_res = list_resp.result.unwrap();
    assert_eq!(list_res["status"], "success");
    assert!(list_res["worktrees"].is_array());

    // 2. worktree_create + worktree_remove RPC
    let create_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "worktree_create",
            "arguments": {
                "repo": repo_root.to_str().unwrap(),
                "branch": "wt/rpc-remove"
            }
        })),
    };
    let create_resp = handle_json_rpc(create_req);
    assert!(create_resp.error.is_none());

    let remove_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(3)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "worktree_remove",
            "arguments": {
                "repo": repo_root.to_str().unwrap(),
                "branch": "wt/rpc-remove"
            }
        })),
    };
    let remove_resp = handle_json_rpc(remove_req);
    assert!(remove_resp.error.is_none());
    let remove_res = remove_resp.result.unwrap();
    assert_eq!(remove_res["removed"], true);

    // 3. worktree_clean RPC
    let clean_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(4)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "worktree_clean",
            "arguments": {
                "repo": repo_root.to_str().unwrap()
            }
        })),
    };
    let clean_resp = handle_json_rpc(clean_req);
    assert!(clean_resp.error.is_none());
    let clean_res = clean_resp.result.unwrap();
    assert!(clean_res["cleaned"].is_number());
}
