//! Integration tests for direct state initialization and markdown-on-demand views.

use std::fs;
use std::process::Command;
use superstate_core::models::{ExecutionState, ExecutionStatus};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

#[test]
fn test_state_init_direct_creates_state_without_plan_md() {
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Fix parser bug",
            "--verify-cmd",
            "cargo test",
            "--no-worktree",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state init");

    assert!(output.status.success(), "init direct failed: {:?}", output);

    // Assert that no plan.md exists on disk
    let plan_path = dir.path().join("plan.md");
    assert!(!plan_path.exists(), "plan.md should not exist on disk");

    // Read and verify state.json
    let state_file = dir.path().join(".agent-state").join("state.json");
    assert!(state_file.exists(), ".agent-state/state.json must exist");

    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();

    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.step, 1);
    assert!(state.no_worktree);

    let active = state
        .active_task
        .as_ref()
        .expect("Active task must be present");
    assert_eq!(active.id, "task-1");
    assert_eq!(active.title, "Fix parser bug");
    assert_eq!(active.verification_cmd.as_deref(), Some("cargo test"));
}

#[test]
fn test_state_init_direct_missing_verify_cmd_fails() {
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(bin())
        .args(["state", "init", "--goal", "Fix parser bug"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state init");

    assert!(
        !output.status.success(),
        "Command should fail when --verify-cmd is missing"
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--verify-cmd is required when initializing with --goal"),
        "stderr should mention --verify-cmd required: {}",
        stderr
    );
}

#[test]
fn test_state_init_missing_plan_and_goal_fails() {
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(bin())
        .args(["state", "init"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state init");

    assert!(
        !output.status.success(),
        "Command should fail when neither --plan nor --goal is provided"
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("Must provide either --plan <path> or --goal <text> with --verify-cmd <cmd>"),
        "stderr should mention either plan or goal is required: {}",
        stderr
    );
}

#[test]
fn test_state_show_renders_markdown_view() {
    let dir = tempfile::tempdir().unwrap();

    // Direct init first
    let init_out = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Fix parser bug",
            "--verify-cmd",
            "cargo test",
            "--no-worktree",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state init");
    assert!(init_out.status.success());

    // Test superstate state show
    let show_out = Command::new(bin())
        .args(["state", "show"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state show");

    assert!(
        show_out.status.success(),
        "state show failed: {:?}",
        show_out
    );
    let stdout = String::from_utf8_lossy(&show_out.stdout);

    assert!(
        stdout.contains("InProgress"),
        "Show output should contain status InProgress: {}",
        stdout
    );
    assert!(
        stdout.contains("Fix parser bug"),
        "Show output should contain active task: {}",
        stdout
    );
    assert!(
        stdout.contains("cargo test"),
        "Show output should contain verification cmd: {}",
        stdout
    );

    // Test superstate state show --markdown
    let show_md_out = Command::new(bin())
        .args(["state", "show", "--markdown"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state show --markdown");

    assert!(show_md_out.status.success());
    let md_stdout = String::from_utf8_lossy(&show_md_out.stdout);
    assert!(md_stdout.contains("Fix parser bug"));

    // Test superstate state show --json
    let show_json_out = Command::new(bin())
        .args(["state", "show", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state show --json");

    assert!(show_json_out.status.success());
    let json_stdout = String::from_utf8_lossy(&show_json_out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_stdout).expect("Valid json expected");
    assert_eq!(parsed["status"], "in_progress");
}
