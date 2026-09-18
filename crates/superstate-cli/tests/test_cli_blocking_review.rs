use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

fn init_git_repo(path: &Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()
        .expect("git init");

    Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(path)
        .output()
        .expect("git config user.name");

    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("git config user.email");

    fs::write(path.join("README.md"), "initial baseline\n").expect("write README.md");
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "chore: initial baseline", "--no-gpg-sign"])
        .current_dir(path)
        .output()
        .expect("git commit");
}

#[test]
fn test_cli_daemon_help_has_review_blocking() {
    // Check `superstate state daemon --help`
    let output = Command::new(bin())
        .args(["state", "daemon", "--help"])
        .output()
        .expect("run superstate state daemon --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--review-blocking"),
        "`superstate state daemon --help` should mention --review-blocking, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains(
            "Treat Critical and High severity review findings as blocking failures requiring retry"
        ),
        "`superstate state daemon --help` should describe --review-blocking, got:\n{}",
        stdout
    );

    // Also check `superstate daemon --help`
    let output_alias = Command::new(bin())
        .args(["daemon", "--help"])
        .output()
        .expect("run superstate daemon --help");
    assert!(output_alias.status.success());
    let stdout_alias = String::from_utf8_lossy(&output_alias.stdout);
    assert!(
        stdout_alias.contains("--review-blocking"),
        "`superstate daemon --help` should mention --review-blocking, got:\n{}",
        stdout_alias
    );
}

#[test]
fn test_cli_daemon_accepts_review_blocking_flag() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    let state_dir = repo.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![superstate_core::models::MilestoneSpec {
        title: "Test Task".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
    }];
    let state = superstate_core::state_machine::init_state_milestones(
        Some("Test Goal"),
        milestones,
        None,
        true,
    )
    .unwrap();
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    let mock_driver =
        "echo '{\"summary\":\"Clean\",\"findings\":[],\"has_blocking_issues\":false}'";

    // Call with --review-blocking
    let output = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            state_path.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--review-blocking",
        ])
        .output()
        .expect("run superstate state daemon with --review-blocking");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unexpected argument '--review-blocking'"),
        "CLI must accept --review-blocking, got stderr:\n{}",
        stderr
    );
}

#[test]
fn test_cli_review_without_args_reviews_last_commit_on_clean_working_tree() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Make a second commit modifying README.md
    fs::write(repo.join("README.md"), "second commit content\n").expect("write update");
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo)
        .output()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "feat: second commit", "--no-gpg-sign"])
        .current_dir(repo)
        .output()
        .expect("git commit");

    // Working tree is completely clean now.
    // Calling `superstate review` without commit_range should review HEAD~1..HEAD
    let mock_driver = "echo '{\"summary\":\"Inspected last commit\",\"has_blocking_issues\":false,\"findings\":[]}'";

    let output = Command::new(bin())
        .args([
            "review",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--json",
        ])
        .output()
        .expect("run superstate review");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Inspected last commit"),
        "superstate review without args should review HEAD~1..HEAD instead of clean early-exit, got:\n{}",
        stdout
    );
}
