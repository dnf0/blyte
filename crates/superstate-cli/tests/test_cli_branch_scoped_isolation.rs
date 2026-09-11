//! Integration tests for branch-scoped state isolation, worktree reverse lookup,
//! and `superstate state list` subcommand.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
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
    run(&["commit", "-m", "chore: initial baseline"]);
}

#[test]
fn test_cli_init_on_feature_branch_creates_branch_scoped_state() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Create and checkout feature branch
    let status = Command::new("git")
        .args(["checkout", "-b", "feat/payment"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    // Run state init without --state
    let output = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Add Stripe",
            "--verify-cmd",
            "true",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state init");

    assert!(
        output.status.success(),
        "state init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Assert that .agent-state/branches/feat-payment.json exists
    let branch_state = dir
        .path()
        .join(".agent-state")
        .join("branches")
        .join("feat-payment.json");
    assert!(
        branch_state.exists(),
        "Expected branch-scoped state file at {}",
        branch_state.display()
    );

    // Assert that .agent-state/state.json was NOT created
    let default_state = dir.path().join(".agent-state").join("state.json");
    assert!(
        !default_state.exists(),
        "Default state.json should not exist on a feature branch"
    );

    // Verify turnkey prompt contains State File and Next Gate Command
    let prompt_out = Command::new(bin())
        .args(["state", "prompt"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state prompt");

    assert!(prompt_out.status.success());
    let prompt_text = String::from_utf8_lossy(&prompt_out.stdout);
    assert!(
        prompt_text.contains("- State File:") || prompt_text.contains("- **State File:**"),
        "Prompt should display state file path: {}",
        prompt_text
    );
    assert!(
        prompt_text.contains("feat-payment.json"),
        "Prompt should reference feat-payment.json: {}",
        prompt_text
    );
    assert!(
        prompt_text.contains("- Next Gate Command:")
            || prompt_text.contains("- **Next Gate Command:**"),
        "Prompt should display next gate command: {}",
        prompt_text
    );
    assert!(
        prompt_text.contains("superstate state next --state"),
        "Prompt should instruct runner how to advance: {}",
        prompt_text
    );
}

#[test]
fn test_cli_worktree_reverse_lookup() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    let status = Command::new("git")
        .args(["checkout", "-b", "feat/payment"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    let init_out = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Add Stripe",
            "--verify-cmd",
            "true",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state init");
    assert!(init_out.status.success());

    let wt_path = dir.path().join(".wt").join("feat-payment-task-1");
    assert!(
        wt_path.exists(),
        "Worktree .wt/feat-payment-task-1 should exist after init"
    );

    // Run `state show --json` inside .wt/task-1 without --state
    let show_out = Command::new(bin())
        .args(["state", "show", "--json"])
        .current_dir(&wt_path)
        .output()
        .expect("Failed to run superstate state show from inside worktree");

    assert!(
        show_out.status.success(),
        "state show in worktree failed: {}",
        String::from_utf8_lossy(&show_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&show_out.stdout);
    let state_json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(state_json["active_task"]["id"], "task-1");
    assert_eq!(state_json["active_task"]["title"], "Add Stripe");

    // Run `state next` inside .wt/task-1 without --state
    let next_out = Command::new(bin())
        .args(["state", "next", "--verify-cmd", "true"])
        .current_dir(&wt_path)
        .output()
        .expect("Failed to run superstate state next from inside worktree");

    assert!(
        next_out.status.success(),
        "state next in worktree failed: {}",
        String::from_utf8_lossy(&next_out.stderr)
    );

    // Verify state file was updated
    let branch_state = dir
        .path()
        .join(".agent-state")
        .join("branches")
        .join("feat-payment.json");
    let content = fs::read_to_string(&branch_state).unwrap();
    let state_reloaded: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(state_reloaded["status"], "completed");
}

#[test]
fn test_cli_state_list() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // Branch 1: feat/payment
    let status = Command::new("git")
        .args(["checkout", "-b", "feat/payment"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    let init1 = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Add Stripe",
            "--verify-cmd",
            "true",
            "--no-worktree",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(init1.status.success());

    // Branch 2: feat/analytics
    let status2 = Command::new("git")
        .args(["checkout", "-b", "feat/analytics"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status2.success());

    let init2 = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Export Analytics",
            "--verify-cmd",
            "true",
            "--no-worktree",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(init2.status.success());

    // Run `superstate state list` tabular output
    let list_out = Command::new(bin())
        .args(["state", "list"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state list");

    assert!(
        list_out.status.success(),
        "state list failed: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let table_str = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        table_str.contains("feat-payment") || table_str.contains("feat/payment"),
        "Table should contain payment branch: {}",
        table_str
    );
    assert!(
        table_str.contains("feat-analytics") || table_str.contains("feat/analytics"),
        "Table should contain analytics branch: {}",
        table_str
    );
    assert!(
        table_str.contains("InProgress"),
        "Table should show InProgress status: {}",
        table_str
    );
    assert!(
        table_str.contains("task-1"),
        "Table should show active task: {}",
        table_str
    );

    // Run `superstate state list --json`
    let json_out = Command::new(bin())
        .args(["state", "list", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state list --json");

    assert!(json_out.status.success());
    let json_str = String::from_utf8_lossy(&json_out.stdout);
    let summaries: serde_json::Value = serde_json::from_str(&json_str).expect("Valid JSON array");
    let arr = summaries.as_array().expect("Should be array");
    assert_eq!(arr.len(), 2, "Expected 2 branch summaries");

    let branches: Vec<&str> = arr
        .iter()
        .filter_map(|item| item["branch"].as_str())
        .collect();
    assert!(
        branches.iter().any(|&b| b.contains("payment")),
        "Expected payment branch in JSON summaries: {:?}",
        branches
    );
    assert!(
        branches.iter().any(|&b| b.contains("analytics")),
        "Expected analytics branch in JSON summaries: {:?}",
        branches
    );

    for item in arr {
        assert_eq!(item["status"], "in_progress");
        assert!(item["active_task"].is_string() || item["active_task"].is_object());
    }
}
