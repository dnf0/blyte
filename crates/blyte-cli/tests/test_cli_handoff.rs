use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

fn init_git_repo(path: &std::path::Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();

    fs::write(path.join("README.md"), "initial").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(path)
        .output()
        .unwrap();
}

#[test]
fn test_cli_handoff_interactive_creates_handoff_package() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();
    init_git_repo(repo_root);

    let output = Command::new(bin())
        .args([
            "state",
            "handoff",
            "--goal",
            "Implement feature Z",
            "--verify-cmd",
            "true",
            "--from",
            "claude",
            "--to",
            "agy",
            "--no-worktree",
        ])
        .current_dir(repo_root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Handoff package written"),
        "Expected 'Handoff package written' in stdout: {}",
        stdout
    );
    assert!(
        stdout.contains("Target Agent: agy"),
        "Expected 'Target Agent: agy' in stdout: {}",
        stdout
    );

    let handoff_path = repo_root.join(".agent-state/handoff.md");
    assert!(handoff_path.exists(), "handoff.md must exist on disk");
    let handoff_content = fs::read_to_string(&handoff_path).unwrap();
    assert!(
        handoff_content.contains("- **Source Agent**: claude"),
        "handoff.md must contain 'Source Agent: claude'"
    );
    assert!(
        handoff_content.contains("- **Target Agent**: agy"),
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
        "Scoped handoff file {}.md must exist",
        slug
    );
}

#[test]
fn test_cli_handoff_json() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();
    init_git_repo(repo_root);

    let output = Command::new(bin())
        .args([
            "state",
            "handoff",
            "--goal",
            "Implement feature Z JSON",
            "--verify-cmd",
            "true",
            "--from",
            "claude",
            "--to",
            "agy",
            "--no-worktree",
            "--json",
        ])
        .current_dir(repo_root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    assert_eq!(val["status"], "ready");
    assert!(val["handoff_file"].is_string());
    assert!(val["handoff_file"].as_str().unwrap().contains("handoff.md"));
    assert!(val["target_resumption_command"].is_string());
    assert!(val["target_resumption_command"]
        .as_str()
        .unwrap()
        .contains("agy"));
    assert!(val["slug"].is_string());
    assert!(!val["slug"].as_str().unwrap().is_empty());

    let slug = val["slug"].as_str().unwrap();
    let scoped_path = repo_root
        .join(".agent-state/handoffs")
        .join(format!("{}.md", slug));
    assert!(
        scoped_path.exists(),
        "Scoped handoff file for slug '{}' must exist at {}",
        slug,
        scoped_path.display()
    );
}

#[test]
fn test_cli_handoff_detach() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();
    init_git_repo(repo_root);

    let output = Command::new(bin())
        .args([
            "state",
            "handoff",
            "--detach",
            "--driver",
            "echo driver-test",
            "--goal",
            "Test Detach",
            "--no-worktree",
        ])
        .current_dir(repo_root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Blyte daemon spawned in background") || stdout.contains("daemon"),
        "Stdout should report daemon status: {}",
        stdout
    );
    assert!(
        stdout.contains("PID:"),
        "Stdout should report PID: {}",
        stdout
    );

    // Cancel daemon to avoid leaving orphaned background process
    let _ = Command::new(bin())
        .args(["state", "cancel", "--all"])
        .current_dir(repo_root)
        .output();
}
