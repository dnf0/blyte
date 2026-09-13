use std::time::Duration;
use superstate_mcp::server::handle_tool_call;
use tempfile::tempdir;

fn setup_test_repo(branch_name: &str) -> tempfile::TempDir {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let _ = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Superstate Test"])
        .current_dir(repo_root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "superstate@test.local"])
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

    let res = handle_tool_call("superstate_handoff", &args)
        .expect("superstate_handoff with detach should succeed");

    assert_eq!(res["status"], "detached");
    assert!(res["job_id"].is_string());
    assert!(res["pid"].is_number());

    let state_path = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-detached-test.json");
    assert!(state_path.exists(), "State file must exist");

    let pid_file = repo_root
        .join(".agent-state")
        .join("branches")
        .join("daemon.pid");
    let fallback_pid_file = repo_root.join(".agent-state").join("daemon.pid");
    assert!(
        pid_file.exists() || fallback_pid_file.exists(),
        "daemon.pid must exist"
    );

    // Let the daemon run briefly
    std::thread::sleep(Duration::from_millis(50));
}
