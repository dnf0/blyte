use blyte_mcp::server::handle_tool_call;
use std::process::Command;
use std::time::Duration;
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

fn setup_test_repo(branch_name: &str) -> tempfile::TempDir {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let _ = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Blyte Test"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "blyte@test.local"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args([
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "chore: baseline",
        ])
        .current_dir(repo_root)
        .status();
    if branch_name != "main" {
        let _ = std::process::Command::new("git")
            .args(["checkout", "-b", branch_name])
            .current_dir(repo_root)
            .status();
    }
    temp
}

#[test]
fn test_mcp_detached_handoff_lifecycle() {
    let temp = setup_test_repo("feat/detached-test");
    let repo_root = temp.path();

    let args = serde_json::json!({
        "goal": "Test detached autonomous run",
        "milestones": [
            { "title": "Milestone 1", "verify_cmd": "sleep 0.1 && true" },
            { "title": "Milestone 2", "verify_cmd": "sleep 0.1 && true" }
        ],
        "detach": true,
        "driver": "echo 'headless coding driver'",
        "repo_root": repo_root.to_str().unwrap()
    });

    let res =
        handle_tool_call("blyte_handoff", &args).expect("blyte_handoff with detach should succeed");

    assert_eq!(res["status"], "detached");
    assert_eq!(res["slug"], "feat-detached-test");
    assert!(res["job_id"].is_string());
    assert!(res["pid"].is_number());
    assert!(res["log_path"].is_string());

    let _guard = res["pid"].as_i64().map(|p| ProcessGuard::new(p as i32));

    let state_path = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-detached-test.json");
    assert!(state_path.exists(), "State file must exist");

    let scoped_pid_file = repo_root
        .join(".agent-state")
        .join("daemons")
        .join("feat-detached-test.pid");
    let pid_file = repo_root
        .join(".agent-state")
        .join("branches")
        .join("daemon.pid");
    let fallback_pid_file = repo_root.join(".agent-state").join("daemon.pid");
    assert!(
        scoped_pid_file.exists() || pid_file.exists() || fallback_pid_file.exists(),
        "daemon pid file must exist"
    );

    // Let the daemon run briefly
    std::thread::sleep(Duration::from_millis(50));
}
