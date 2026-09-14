use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use superstate_mcp::server::handle_tool_call;
use tempfile::tempdir;

struct ProcessGuard(Option<i32>);

impl ProcessGuard {
    fn new(pid: i32) -> Self {
        Self(Some(pid))
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

fn init_git_repo(path: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    };

    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Test User"]);
    run(&["config", "user.email", "test@example.com"]);

    fs::write(path.join("README.md"), b"# Base\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "--no-gpg-sign", "-m", "chore: initial baseline"]);
}

#[test]
fn test_mcp_multi_daemon_fleet_tracking() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path();
    init_git_repo(repo_root);

    // 1. Checkout feat/alpha and launch detached handoff
    let status_alpha = Command::new("git")
        .args(["checkout", "-b", "feat/alpha"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(status_alpha.success());

    let args_alpha = serde_json::json!({
        "goal": "Build alpha feature",
        "verify_cmd": "true",
        "detach": true,
        "driver": "sleep 10",
        "repo_root": repo_root.to_str().unwrap()
    });

    let res_alpha = handle_tool_call("superstate_handoff", &args_alpha)
        .expect("superstate_handoff on feat/alpha should succeed");

    assert_eq!(res_alpha["status"], "detached");
    assert_eq!(res_alpha["slug"], "feat-alpha");
    assert!(res_alpha["job_id"].is_string());
    assert!(res_alpha["pid"].is_number());
    assert!(res_alpha["log_path"].is_string());

    let alpha_pid = res_alpha["pid"].as_i64().unwrap() as i32;
    let _guard_alpha = ProcessGuard::new(alpha_pid);
    let alpha_log = res_alpha["log_path"].as_str().unwrap().to_string();

    // 2. Checkout feat/beta and launch detached handoff
    let status_beta = Command::new("git")
        .args(["checkout", "-b", "feat/beta"])
        .current_dir(repo_root)
        .status()
        .unwrap();
    assert!(status_beta.success());

    let args_beta = serde_json::json!({
        "goal": "Build beta feature",
        "verify_cmd": "true",
        "detach": true,
        "driver": "sleep 10",
        "repo_root": repo_root.to_str().unwrap()
    });

    let res_beta = handle_tool_call("superstate_handoff", &args_beta)
        .expect("superstate_handoff on feat/beta should succeed");

    assert_eq!(res_beta["status"], "detached");
    assert_eq!(res_beta["slug"], "feat-beta");
    assert!(res_beta["job_id"].is_string());
    assert!(res_beta["pid"].is_number());
    assert!(res_beta["log_path"].is_string());

    let beta_pid = res_beta["pid"].as_i64().unwrap() as i32;
    let _guard_beta = ProcessGuard::new(beta_pid);
    let beta_log = res_beta["log_path"].as_str().unwrap().to_string();

    assert_ne!(alpha_pid, beta_pid, "Daemon PIDs must be distinct");
    assert_ne!(alpha_log, beta_log, "Log paths must be distinct");

    // Allow daemons to stabilize
    std::thread::sleep(Duration::from_millis(50));

    // 3. Verify state_list returns deprecation error directing to superstate_handoff
    let list_args = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });
    let list_err = handle_tool_call("state_list", &list_args).unwrap_err();
    assert!(list_err.contains("deprecated"));
    assert!(list_err.contains("superstate_handoff"));

    // 4. Query superstate_status for branch 1 and branch 2 and verify independent logs
    let status_args_alpha = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap(),
        "state": "branches/feat-alpha.json",
        "json": true
    });
    let status_res_alpha = handle_tool_call("superstate_status", &status_args_alpha)
        .expect("superstate_status on feat-alpha should succeed");
    assert_eq!(status_res_alpha["daemon"]["pid"], alpha_pid);
    assert_eq!(status_res_alpha["daemon"]["running"], true);
    assert_eq!(status_res_alpha["daemon"]["slug"], "feat-alpha");
    assert_eq!(status_res_alpha["daemon"]["log_file"], alpha_log);

    let status_args_beta = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap(),
        "state": "branches/feat-beta.json",
        "json": true
    });
    let status_res_beta = handle_tool_call("superstate_status", &status_args_beta)
        .expect("superstate_status on feat-beta should succeed");
    assert_eq!(status_res_beta["daemon"]["pid"], beta_pid);
    assert_eq!(status_res_beta["daemon"]["running"], true);
    assert_eq!(status_res_beta["daemon"]["slug"], "feat-beta");
    assert_eq!(status_res_beta["daemon"]["log_file"], beta_log);

    // 5. Test superstate_cancel terminates alpha daemon
    let cancel_args_alpha = serde_json::json!({
        "repo_root": repo_root.to_str().unwrap(),
        "slug": "feat-alpha"
    });
    let cancel_res = handle_tool_call("superstate_cancel", &cancel_args_alpha)
        .expect("superstate_cancel should succeed");
    assert_eq!(cancel_res["status"], "cancelled");

    std::thread::sleep(Duration::from_millis(50));

    // Machine-readable format should report running: false
    let status_res_alpha_stopped = handle_tool_call("superstate_status", &status_args_alpha)
        .expect("superstate_status on stopped feat-alpha should succeed");
    assert_eq!(status_res_alpha_stopped["daemon"]["running"], false);
}
