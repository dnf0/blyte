//! Integration tests for Blyte MCP Gateway tools surface.
//!
//! Validates ADR 0002 compliance: strictly 4 Gateway tools exposed over `tools/list`,
//! descriptive deprecation rejection for in-loop state tools, and default detached execution.

use blyte_mcp::server::{handle_json_rpc, handle_tool_call, JsonRpcRequest};
use std::process::Command;
use tempfile::tempdir;

/// RAII process guard to cleanly terminate spawned background daemons on drop.
struct ProcessGuard(Option<i32>);

impl ProcessGuard {
    fn new(pid: i32) -> Self {
        Self(Some(pid))
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // Signal daemon to exit immediately during test teardown
            let _ = Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

/// Initializes a temporary Git repository with an initial commit baseline.
///
/// # Arguments
/// * `branch_name` - Target branch to initialize and checkout.
///
/// # Returns
/// TempDir handle keeping the repository directory alive for test duration.
fn setup_test_repo(branch_name: &str) -> tempfile::TempDir {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    // Initialize git repository with main branch
    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();

    // Configure test author identity for commits
    let _ = Command::new("git")
        .args(["config", "user.name", "Blyte Test"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "blyte@test.local"])
        .current_dir(repo_root)
        .status();

    // Create initial commit baseline
    let _ = Command::new("git")
        .args([
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "chore: baseline",
        ])
        .current_dir(repo_root)
        .status();

    // Check out target branch if different from main
    if branch_name != "main" {
        let _ = Command::new("git")
            .args(["checkout", "-b", branch_name])
            .current_dir(repo_root)
            .status();
    }

    temp
}

/// Verifies that `tools/list` returns strictly the 4 Gateway tools defined in ADR 0002:
/// `blyte_handoff`, `blyte_status`, `blyte_cancel`, `blyte_doctor`.
#[test]
fn test_tools_list_returns_strictly_four_gateway_tools() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: "tools/list".to_string(),
        params: None,
    };

    let resp = handle_json_rpc(req);
    assert!(
        resp.error.is_none(),
        "tools/list should not error: {:?}",
        resp.error
    );

    let result = resp.result.expect("result should be present");
    let tools = result
        .get("tools")
        .and_then(|v| v.as_array())
        .expect("tools array must exist");

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();

    let expected_gateway_tools = [
        "blyte_handoff",
        "blyte_status",
        "blyte_cancel",
        "blyte_doctor",
    ];

    // Assert exact 4 Gateway tools are returned
    assert_eq!(
        tool_names.len(),
        4,
        "tools/list must return exactly 4 Gateway tools, found: {:?}",
        tool_names
    );

    for expected in &expected_gateway_tools {
        assert!(
            tool_names.contains(expected),
            "tools/list missing expected Gateway tool: {}",
            expected
        );
    }
}

/// Verifies that calling legacy in-loop state tools returns descriptive deprecation guidance
/// redirecting clients to autonomous detached execution via `blyte_handoff`.
#[test]
fn test_calling_legacy_tools_returns_deprecation_error() {
    let dummy_args = serde_json::json!({
        "goal": "Test legacy tool deprecation rejection",
        "verify_cmd": "true"
    });

    let legacy_tools_to_probe = ["state_next", "state_init"];

    for tool_name in legacy_tools_to_probe {
        let err = handle_tool_call(tool_name, &dummy_args).expect_err(&format!(
            "Calling legacy tool '{}' must return an error",
            tool_name
        ));

        let expected_err = format!(
            "The tool '{}' has been deprecated. In-loop MCP execution is an anti-pattern. Use 'blyte_handoff' to initiate autonomous detached execution in an isolated worktree.",
            tool_name
        );

        assert_eq!(
            err, expected_err,
            "Legacy tool '{}' did not produce the expected deprecation message",
            tool_name
        );
    }
}

/// Verifies that `blyte_handoff` defaults to `detach: true` when the `detach` parameter
/// is omitted from input arguments.
#[test]
fn test_blyte_handoff_defaults_to_detach_true() {
    let temp = setup_test_repo("feat/gateway-default-detach");
    let repo_root = temp.path();

    // Call blyte_handoff WITHOUT passing "detach" parameter
    let args = serde_json::json!({
        "goal": "Verify default detached execution",
        "milestones": [
            { "title": "Milestone 1", "verify_cmd": "sleep 0.1 && true" }
        ],
        "driver": "echo 'headless coding driver'",
        "repo_root": repo_root.to_str().unwrap()
    });

    let res = handle_tool_call("blyte_handoff", &args)
        .expect("blyte_handoff without explicit detach parameter should succeed");

    // Ensure spawned daemon is reaped on drop
    let _guard = res["pid"].as_i64().map(|p| ProcessGuard::new(p as i32));

    // Status MUST be "detached", proving detach defaults to true
    assert_eq!(
        res["status"], "detached",
        "blyte_handoff must default to detached execution"
    );
    assert_eq!(res["slug"], "feat-gateway-default-detach");
    assert!(res["job_id"].is_string());
    assert!(res["pid"].is_number());
}

/// Verifies that `blyte_status` returns structured status and `blyte_cancel` cleanly terminates daemons.
#[test]
fn test_blyte_status_and_cancel_lifecycle() {
    let temp = setup_test_repo("feat/status-cancel-test");
    let repo_root = temp.path();

    // Handoff to launch detached worker
    let args = serde_json::json!({
        "goal": "Verify status and cancel",
        "milestones": [
            { "title": "Milestone 1", "verify_cmd": "sleep 1 && true" }
        ],
        "driver": "echo 'headless coding driver'",
        "repo_root": repo_root.to_str().unwrap()
    });
    let handoff_res = handle_tool_call("blyte_handoff", &args).expect("handoff succeeds");
    let pid = handoff_res["pid"].as_i64().expect("pid present") as i32;
    let _guard = ProcessGuard::new(pid);

    // Call blyte_status
    let status_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });
    let status_res = handle_tool_call("blyte_status", &status_args).expect("status succeeds");
    assert_eq!(status_res["daemon"]["pid"], pid);
    assert_eq!(status_res["daemon"]["running"], true);

    // Call blyte_cancel
    let cancel_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap(),
        "slug": "feat-status-cancel-test"
    });
    let cancel_res = handle_tool_call("blyte_cancel", &cancel_args).expect("cancel succeeds");
    assert_eq!(cancel_res["status"], "cancelled");

    // Post-cancellation status should report running: false
    let status_after = handle_tool_call("blyte_status", &status_args).expect("status succeeds");
    assert_eq!(status_after["daemon"]["running"], false);
}

/// Verifies that `blyte_doctor` executes diagnostics and returns passing status.
#[test]
fn test_blyte_doctor_reports_health() {
    let temp = setup_test_repo("feat/doctor-test");
    let repo_root = temp.path();

    let doctor_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap()
    });
    let doctor_res = handle_tool_call("blyte_doctor", &doctor_args).expect("doctor succeeds");
    assert_eq!(doctor_res["status"], "healthy");
    assert_eq!(doctor_res["is_healthy"], true);
    assert!(doctor_res["issues"].is_array());
}
