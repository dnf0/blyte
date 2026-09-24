//! End-to-end integration test for Blyte MCP detached handoff gateway.
//!
//! Validates the complete autonomous lifecycle from an MCP client perspective:
//! 1. Client sends JSON-RPC `tools/call` for `blyte_handoff` with goal and milestone.
//! 2. Worktree and detached background daemon spawn with default isolated settings.
//! 3. Background daemon executes driver, satisfies verification gate, automerges worktree into `main`,
//!    cleans up temporary worktrees, and emits `.agent-state/completion.json`.
//! 4. Client polls status via `blyte_status` confirming completed milestone and clean state.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use blyte_core::daemon::is_pid_alive;
use blyte_mcp::server::{handle_json_rpc, JsonRpcRequest};
use tempfile::tempdir;

/// RAII process guard to terminate background daemon if test panics or fails prematurely.
struct ProcessGuard(Option<i32>);

impl ProcessGuard {
    fn new(pid: i32) -> Self {
        Self(Some(pid))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            if is_pid_alive(pid) {
                #[cfg(unix)]
                let _ = Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                #[cfg(windows)]
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
}

/// Sets up a real git repository with initial commit on `main`.
fn setup_git_repo() -> tempfile::TempDir {
    let temp = tempdir().expect("create temp dir");
    let repo_root = temp.path();

    // Initialize git repository with main branch
    let init_res = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();
    if let Ok(status) = init_res {
        if !status.success() {
            let _ = Command::new("git")
                .arg("init")
                .current_dir(repo_root)
                .status();
            let _ = Command::new("git")
                .args(["checkout", "-b", "main"])
                .current_dir(repo_root)
                .status();
        }
    }

    // Configure test author identity for commits
    let _ = Command::new("git")
        .args(["config", "user.name", "Blyte Test"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "blyte@test.local"])
        .current_dir(repo_root)
        .status();

    // Baseline commit on main branch
    fs::write(repo_root.join("README.md"), "# E2E Test Repository\n").expect("write README");
    let _ = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .args([
            "commit",
            "--no-gpg-sign",
            "-m",
            "chore: initial commit on main",
        ])
        .current_dir(repo_root)
        .status();

    temp
}

#[test]
fn test_e2e_detached_handoff_gateway_lifecycle() {
    let temp = setup_git_repo();
    let repo_root = temp.path();

    // 1. Sends a JSON-RPC tools/call for blyte_handoff via handle_json_rpc:
    //    - goal: "Implement feature end-to-end"
    //    - milestones: [{"title": "Step 1", "verify_cmd": "test -f step1.txt"}]
    //    - driver: A shell script or command that creates step1.txt in the worktree
    //    - Note: worktree: true and detach: true are both default!
    let handoff_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "blyte_handoff",
            "arguments": {
                "goal": "Implement feature end-to-end",
                "milestones": [
                    {
                        "title": "Step 1",
                        "verify_cmd": "test -f step1.txt"
                    }
                ],
                "driver": "echo 'step1 completed' > step1.txt",
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };

    let handoff_resp = handle_json_rpc(handoff_req);
    assert!(
        handoff_resp.error.is_none(),
        "blyte_handoff failed with error: {:?}",
        handoff_resp.error
    );

    let handoff_res = handoff_resp
        .result
        .expect("blyte_handoff result must be present");

    // 2. Verifies blyte_handoff response:
    //    - Returns status: "detached"
    //    - Returns slug, job_id, pid, log_path
    //    - PID is alive or process ran
    assert_eq!(
        handoff_res["status"], "detached",
        "blyte_handoff must return status 'detached'"
    );
    assert_eq!(
        handoff_res["slug"], "default",
        "default handoff slug should be 'default'"
    );
    assert!(
        handoff_res["job_id"].is_string(),
        "job_id must be a string identifier"
    );
    assert!(
        handoff_res["pid"].is_number(),
        "pid must be an integer process id"
    );
    assert!(
        handoff_res["log_path"].is_string(),
        "log_path must be a string file path"
    );

    let pid = handoff_res["pid"]
        .as_i64()
        .expect("pid must be a valid integer") as i32;
    assert!(pid > 0, "PID must be a positive integer");

    let mut guard = ProcessGuard::new(pid);

    let log_path = PathBuf::from(
        handoff_res["log_path"]
            .as_str()
            .expect("log_path must be valid string"),
    );

    // 3. Waits for the detached daemon process to finish executing (poll PID liveness up to 10s)
    let start = Instant::now();
    let timeout = Duration::from_secs(10);
    while is_pid_alive(pid) {
        if start.elapsed() > timeout {
            panic!(
                "Detached daemon (PID {}) timed out after {:?}. Log contents:\n{}",
                pid,
                timeout,
                fs::read_to_string(&log_path).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Daemon exited normally, disarm guard
    guard.disarm();

    // Allow a brief moment for OS file buffers to sync
    std::thread::sleep(Duration::from_millis(100));

    // 4. Verifies:
    //    - Background daemon exited with code 0 (log file clean)
    assert!(
        log_path.exists(),
        "Log file must exist at {}",
        log_path.display()
    );
    let log_content = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !log_content.contains("Daemon execution error"),
        "Log file reported daemon execution error:\n{}",
        log_content
    );
    assert!(
        log_content.contains("Daemon completed"),
        "Log file must record daemon completion:\n{}",
        log_content
    );

    //    - Changes from worktree (step1.txt) were cleanly merged into main!
    let merged_step1 = repo_root.join("step1.txt");
    assert!(
        merged_step1.exists(),
        "Changes from worktree (step1.txt) must be merged into main branch at {}",
        merged_step1.display()
    );
    let step1_text = fs::read_to_string(&merged_step1).expect("read step1.txt");
    assert_eq!(
        step1_text.trim(),
        "step1 completed",
        "step1.txt content must match driver output"
    );

    //    - .agent-state/completion.json exists with status: "completed" and worktree_merged: true
    let completion_file = repo_root.join(".agent-state").join("completion.json");
    assert!(
        completion_file.exists(),
        "Completion artifact must exist at {}",
        completion_file.display()
    );
    let completion_raw = fs::read_to_string(&completion_file).expect("read completion.json");
    let completion_val: serde_json::Value =
        serde_json::from_str(&completion_raw).expect("parse completion json");

    assert_eq!(
        completion_val["status"], "completed",
        "completion report status must be 'completed'"
    );
    assert_eq!(
        completion_val["worktree_merged"], true,
        "completion report must record worktree_merged as true"
    );
    assert_eq!(
        completion_val["milestones_completed"], 1,
        "completion report must record 1 milestone completed"
    );

    //    - Temporary worktree directory .wt/<slug> was removed
    let wt_task1_dir = repo_root.join(".wt").join("task-1");
    assert!(
        !wt_task1_dir.exists(),
        "Temporary worktree directory {} must be removed after merge and completion",
        wt_task1_dir.display()
    );

    // 5. Sends a JSON-RPC tools/call for blyte_status:
    //    - Verifies that status reports completion, milestones_completed: 1, and clean state.
    let status_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "blyte_status",
            "arguments": {
                "repo_root": repo_root.to_str().unwrap(),
                "json": true
            }
        })),
    };

    let status_resp = handle_json_rpc(status_req);
    assert!(
        status_resp.error.is_none(),
        "blyte_status failed with error: {:?}",
        status_resp.error
    );

    let status_res = status_resp
        .result
        .expect("blyte_status result must be present");

    assert_eq!(
        status_res["status"], "completed",
        "blyte_status must report status 'completed'"
    );
    assert_eq!(
        status_res["active_task"],
        serde_json::Value::Null,
        "active_task must be null when all milestones finish"
    );
    assert_eq!(
        status_res["daemon"]["running"], false,
        "daemon should not be running after completion"
    );

    let completed_tasks = status_res["completed_tasks"]
        .as_array()
        .expect("completed_tasks must be an array");
    assert_eq!(
        completed_tasks.len(),
        1,
        "completed_tasks must contain 1 milestone"
    );
    assert_eq!(
        completed_tasks[0]["title"], "Step 1",
        "completed milestone title must match"
    );

    // Verify text representation via blyte_status
    let text_status_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(3)),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "blyte_status",
            "arguments": {
                "repo_root": repo_root.to_str().unwrap()
            }
        })),
    };

    let text_status_resp = handle_json_rpc(text_status_req);
    let text_status_res = text_status_resp
        .result
        .expect("text status result must be present");
    let text = text_status_res["content"][0]["text"]
        .as_str()
        .expect("text content must be string");

    assert!(
        text.contains("State Status: Completed"),
        "Text status must display Completed state status. Got:\n{}",
        text
    );
    assert!(
        text.contains("Completed Tasks: 1"),
        "Text status must display 1 completed task. Got:\n{}",
        text
    );
    assert!(
        text.contains("Step 1"),
        "Text status must display milestone title. Got:\n{}",
        text
    );
}
