use blyte_mcp::server::handle_tool_call;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn setup_test_repo(branch_name: &str) -> tempfile::TempDir {
    let temp = tempdir().unwrap();
    let repo_root = temp.path();

    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "Blyte Test"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "blyte@test.local"])
        .current_dir(repo_root)
        .status();

    fs::write(
        repo_root.join("Cargo.toml"),
        r#"[package]
name = "test-cross-agent"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(repo_root.join("src")).unwrap();
    fs::write(repo_root.join("src/lib.rs"), "// test\n").unwrap();

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .args(["commit", "--no-gpg-sign", "-m", "chore: baseline"])
        .current_dir(repo_root)
        .status();
    if branch_name != "main" {
        let _ = Command::new("git")
            .args(["checkout", "-b", branch_name])
            .current_dir(repo_root)
            .status();
    }
    temp
}

#[test]
fn test_mcp_cross_agent_handoff() {
    let temp = setup_test_repo("feat/cross-agent-pipeline");
    let repo_root = temp.path();

    let args = serde_json::json!({
        "goal": "Build cross-agent pipeline",
        "verify_cmd": "cargo check",
        "from_agent": "claude",
        "to_agent": "agy",
        "no_worktree": true,
        "detach": false,
        "repo_root": repo_root.to_str().unwrap()
    });

    let res =
        handle_tool_call("blyte_handoff", &args).expect("blyte_handoff tool call should succeed");

    assert_eq!(res["status"], "ready");

    let handoff_file = res["handoff_file"]
        .as_str()
        .expect("handoff_file must be present in response JSON");
    assert!(
        handoff_file.contains("handoff.md"),
        "handoff_file should contain 'handoff.md', got: {}",
        handoff_file
    );

    let resumption_cmd = res["target_resumption_command"]
        .as_str()
        .expect("target_resumption_command must be present in response JSON");
    assert!(
        resumption_cmd.contains("agy"),
        "target_resumption_command should contain 'agy', got: {}",
        resumption_cmd
    );

    let turnkey_prompt = res["turnkey_prompt"]
        .as_str()
        .expect("turnkey_prompt must be present in response JSON");
    assert!(
        turnkey_prompt.contains("Build cross-agent pipeline"),
        "turnkey_prompt must contain active milestone details, got: {}",
        turnkey_prompt
    );

    let handoff_path = repo_root.join(".agent-state/handoff.md");
    assert!(
        handoff_path.exists(),
        "handoff.md must exist on disk at {}",
        handoff_path.display()
    );

    let handoff_content = fs::read_to_string(&handoff_path).unwrap();
    assert!(
        handoff_content.contains("- **Source Agent**: claude")
            || handoff_content.contains("Source Agent: claude"),
        "handoff.md must contain 'Source Agent: claude'"
    );
    assert!(
        handoff_content.contains("- **Target Agent**: agy")
            || handoff_content.contains("Target Agent: agy"),
        "handoff.md must contain 'Target Agent: agy'"
    );

    let handoffs_dir = repo_root.join(".agent-state/handoffs");
    assert!(handoffs_dir.exists(), "handoffs dir must exist");

    let slug_line = handoff_content
        .lines()
        .find(|l| l.contains("**Job Slug**:"));
    assert!(
        slug_line.is_some(),
        "Job Slug must be present in handoff.md"
    );
    let slug = slug_line
        .unwrap()
        .split("**Job Slug**:")
        .nth(1)
        .unwrap()
        .trim();

    let scoped_file = handoffs_dir.join(format!("{}.md", slug));
    assert!(
        scoped_file.exists(),
        "Scoped handoff file {}.md must exist at {}",
        slug,
        scoped_file.display()
    );

    let scoped_content = fs::read_to_string(&scoped_file).unwrap();
    assert!(
        scoped_content.contains("- **Source Agent**: claude")
            || scoped_content.contains("Source Agent: claude"),
        "Scoped handoff file must contain 'Source Agent: claude'"
    );
    assert!(
        scoped_content.contains("- **Target Agent**: agy")
            || scoped_content.contains("Target Agent: agy"),
        "Scoped handoff file must contain 'Target Agent: agy'"
    );
}
