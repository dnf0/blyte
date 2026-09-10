//! Integration tests for worktree provisioning and contract injection in `state prompt`.

use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_cli::commands::state::{cmd_init, cmd_prompt};
use superstate_core::state_machine::load_state;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

fn init_git_repo(path: &Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .status()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .status()
        .unwrap();

    fs::write(path.join("README.md"), b"# Base\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "chore: initial baseline"])
        .current_dir(path)
        .status()
        .unwrap();
}

#[test]
fn test_state_prompt_provisions_worktree_and_contract() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    let plan_file = dir.path().join("plan.md");
    fs::write(
        &plan_file,
        "### Task 1: First task\n- **Depends on:** none\n- Files to modify: src/task1.rs\n\n\
         ### Task 2: Second task\n- **Depends on:** Task 1\n- Files to modify: src/task2.rs\n",
    )
    .unwrap();

    let state_file = dir.path().join(".agent-state").join("state.json");
    cmd_init(plan_file, state_file.clone(), false).expect("Failed to initialize state");

    // Call cmd_prompt with 5 arguments: (state_path, task, with_graph, json_output, no_worktree)
    cmd_prompt(&state_file, None, false, false, false).expect("cmd_prompt failed");

    let expected_wt_path = dir.path().join(".wt").join("task-1");
    assert!(
        expected_wt_path.exists(),
        "Worktree directory .wt/task-1 should exist after cmd_prompt"
    );
    assert!(
        expected_wt_path.is_dir(),
        ".wt/task-1 should be a directory"
    );

    // Reload state.json and verify worktree and branch are recorded
    let state = load_state(&state_file).expect("Failed to reload state");
    assert_eq!(
        state.active_worktree,
        Some(expected_wt_path.clone()),
        "state.active_worktree should point to the provisioned worktree"
    );
    assert_eq!(
        state.active_branch,
        Some("wt/task-1".to_string()),
        "state.active_branch should be wt/task-1"
    );

    // Test CLI invocation with --json flag
    let output = Command::new(bin())
        .args([
            "state",
            "prompt",
            "--state",
            state_file.to_str().unwrap(),
            "--json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state prompt --json");

    assert!(
        output.status.success(),
        "superstate state prompt --json failed: {:?}",
        output
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Failed to parse JSON output from state prompt");

    assert_eq!(json["task"]["id"], "Task 1");
    assert_eq!(
        json["active_worktree"].as_str(),
        Some(expected_wt_path.to_str().unwrap())
    );
    assert_eq!(json["active_branch"], "wt/task-1");

    let prompt_text = json["prompt"]
        .as_str()
        .expect("json['prompt'] must be a string");
    assert!(
        prompt_text.contains("## 🌲 Active Isolated Worktree"),
        "Prompt should contain active isolated worktree section: {}",
        prompt_text
    );
    assert!(
        prompt_text.contains(&format!("- Worktree Path: {}", expected_wt_path.display())),
        "Prompt should contain worktree path: {}",
        prompt_text
    );
    assert!(
        prompt_text.contains("- Branch: wt/task-1"),
        "Prompt should contain active branch: {}",
        prompt_text
    );
    assert!(
        prompt_text.contains(
            "- Working Directory Contract: All code edits, test runs, and git commits MUST be performed inside this worktree!"
        ),
        "Prompt should contain working directory contract: {}",
        prompt_text
    );
}

#[test]
fn test_state_prompt_provisions_worktree_from_state_json() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    let state_dir = dir.path().join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_file = state_dir.join("state.json");

    let initial_state = superstate_core::models::ExecutionState {
        status: superstate_core::models::ExecutionStatus::InProgress,
        step: 0,
        active_task: Some(superstate_core::models::TaskSpec {
            id: "task-1".to_string(),
            title: "First milestone".to_string(),
            depends_on: vec![],
            files_modify: vec![std::path::PathBuf::from("src/task1.rs")],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }),
        active_worktree: None,
        active_branch: None,
        pending_tasks: vec![superstate_core::models::TaskSpec {
            id: "task-2".to_string(),
            title: "Second milestone".to_string(),
            depends_on: vec!["task-1".to_string()],
            files_modify: vec![],
            interfaces_produces: vec![],
            verification_cmd: None,
        }],
        completed_tasks: vec![],
        modified_files: vec![],
        errors: vec![],
        plan_path: None,
        notes: vec![],
        no_worktree: false,
    };
    superstate_core::state_machine::save_state_atomic(&state_file, &initial_state).unwrap();

    // Call cmd_prompt
    cmd_prompt(&state_file, None, false, false, false).expect("cmd_prompt failed");

    let expected_wt_path = dir.path().join(".wt").join("task-1");
    assert!(expected_wt_path.exists());
    assert!(expected_wt_path.is_dir());

    let reloaded = load_state(&state_file).unwrap();
    assert_eq!(reloaded.active_worktree, Some(expected_wt_path.clone()));
    assert_eq!(
        reloaded.active_branch,
        Some("wt/task-1".to_string())
    );

    // Test --json flag via CLI
    let output = Command::new(bin())
        .args([
            "state",
            "prompt",
            "--state",
            state_file.to_str().unwrap(),
            "--json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state prompt --json");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(json["task"]["id"], "task-1");
    assert_eq!(
        json["active_worktree"].as_str(),
        Some(expected_wt_path.to_str().unwrap())
    );
    assert_eq!(json["active_branch"], "wt/task-1");
}

#[test]
fn test_state_prompt_non_git_json_fallback() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_file = state_dir.join("state.json");

    let initial_state = superstate_core::models::ExecutionState {
        status: superstate_core::models::ExecutionStatus::InProgress,
        step: 0,
        active_task: Some(superstate_core::models::TaskSpec {
            id: "task-1".to_string(),
            title: "Non-git task".to_string(),
            depends_on: vec![],
            files_modify: vec![std::path::PathBuf::from("src/task1.rs")],
            interfaces_produces: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }),
        active_worktree: None,
        active_branch: None,
        pending_tasks: vec![],
        completed_tasks: vec![],
        modified_files: vec![],
        errors: vec![],
        plan_path: None,
        notes: vec![],
        no_worktree: false,
    };
    superstate_core::state_machine::save_state_atomic(&state_file, &initial_state).unwrap();

    // Call cmd_prompt directly
    cmd_prompt(&state_file, None, false, false, false)
        .expect("cmd_prompt should succeed in non-git directory");

    // Call state prompt with --json flag
    let output = Command::new(bin())
        .args([
            "state",
            "prompt",
            "--state",
            state_file.to_str().unwrap(),
            "--json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state prompt --json");

    assert!(
        output.status.success(),
        "superstate state prompt --json failed: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Failed to parse JSON output");

    assert!(
        json["active_worktree"].is_null(),
        "active_worktree should be null in non-git directory"
    );
    assert!(
        json["active_branch"].is_null(),
        "active_branch should be null in non-git directory"
    );

    let prompt_text = json["prompt"].as_str().expect("prompt must be a string");
    assert!(
        !prompt_text.contains("Active Isolated Worktree"),
        "Prompt should not include worktree section in non-git directory: {}",
        prompt_text
    );
}

#[test]
fn test_state_prompt_no_worktree_flag() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    let plan_content = r#"# Sample Plan
### Task 1: First In-Place Task
- Depends on: none
Files to modify:
- `src/task1.rs`
Verification command: `cargo test`

Implement the first task.
"#;
    let plan_file = dir.path().join("plan.md");
    fs::write(&plan_file, plan_content).unwrap();

    let state_file = dir.path().join(".agent-state/state.json");
    cmd_init(plan_file, state_file.clone(), false).expect("Failed to initialize state");

    // Call state prompt with --no-worktree and --json flag
    let output = Command::new(bin())
        .args([
            "state",
            "prompt",
            "--state",
            state_file.to_str().unwrap(),
            "--no-worktree",
            "--json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run superstate state prompt --no-worktree --json");

    assert!(
        output.status.success(),
        "superstate state prompt --no-worktree --json failed: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Failed to parse JSON output");

    assert!(
        json["active_worktree"].is_null(),
        "active_worktree must be null when --no-worktree is passed"
    );
    assert!(
        json["active_branch"].is_null(),
        "active_branch must be null when --no-worktree is passed"
    );

    let wt_dir = dir.path().join(".wt");
    assert!(
        !wt_dir.exists(),
        ".wt directory must NOT be created when --no-worktree is passed"
    );

    let prompt_text = json["prompt"].as_str().expect("prompt must be a string");
    assert!(
        !prompt_text.contains("Active Isolated Worktree"),
        "Prompt should not include worktree section when --no-worktree is passed"
    );
    assert!(
        !prompt_text.contains("inside this worktree"),
        "Prompt should not mention committing inside worktree when in-place"
    );
}

