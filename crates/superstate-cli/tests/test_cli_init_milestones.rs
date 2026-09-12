//! Integration tests for CLI milestone-based direct initialization and sequential worktree progression.

use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_core::models::{ExecutionState, ExecutionStatus};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

fn init_git_repo(path: &Path, branch: &str) {
    let status = Command::new("git")
        .args(["init", "-b", branch])
        .current_dir(path)
        .status()
        .expect("git init failed");
    assert!(status.success());

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

    fs::write(path.join("README.md"), b"# Base Repository\n").unwrap();
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
fn test_cli_init_milestones_from_json_string() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "feat/cli-feature");

    let json_milestones =
        r#"[{"title":"Step 1","verify_cmd":"true"},{"title":"Step 2","verify_cmd":"true"}]"#;
    let output = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            json_milestones,
            "--worktree",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run superstate state init");

    assert!(output.status.success(), "init failed: {:?}", output);

    let state_file = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-cli-feature.json");
    assert!(
        state_file.exists(),
        ".agent-state/branches/feat-cli-feature.json must exist"
    );

    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();

    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.step, 1);
    assert_eq!(state.base_branch.as_deref(), Some("feat/cli-feature"));

    let active = state
        .active_task
        .as_ref()
        .expect("Active task must be present");
    assert_eq!(active.id, "task-1");
    assert_eq!(active.title, "Step 1");
    assert_eq!(active.verification_cmd.as_deref(), Some("true"));

    assert_eq!(state.pending_tasks.len(), 1);
    assert_eq!(state.pending_tasks[0].id, "task-2");
    assert_eq!(state.pending_tasks[0].title, "Step 2");
    assert_eq!(state.pending_tasks[0].depends_on, vec!["task-1"]);

    let wt_task_1 = repo_root.join(".wt").join("feat-cli-feature-task-1");
    assert!(
        wt_task_1.exists(),
        ".wt/feat-cli-feature-task-1 directory must exist"
    );
    assert_eq!(state.active_worktree, Some(wt_task_1));
    assert_eq!(
        state.active_branch,
        Some("wt/feat-cli-feature-task-1".to_string())
    );

    // Verify superstate state show displays Base Branch
    let show_output = Command::new(bin())
        .args(["state", "show"])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run superstate state show");
    assert!(show_output.status.success());
    let stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(
        stdout.contains("- **Base Branch:** `feat/cli-feature`"),
        "show output should contain base branch: {}",
        stdout
    );
}

#[test]
fn test_cli_init_milestones_from_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "feat/cli-feature");

    let json_milestones =
        r#"[{"title":"Step 1","verify_cmd":"true"},{"title":"Step 2","verify_cmd":"true"}]"#;
    let mut child = Command::new(bin())
        .args(["state", "init", "--milestones", "-"])
        .current_dir(&repo_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn superstate");

    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("Failed to open stdin");
        stdin
            .write_all(json_milestones.as_bytes())
            .expect("Failed to write to stdin");
    }

    let output = child.wait_with_output().expect("Failed to wait on child");
    assert!(
        output.status.success(),
        "init from stdin failed: {:?}",
        output
    );

    let state_file = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-cli-feature.json");
    assert!(state_file.exists());
    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();

    assert_eq!(state.base_branch.as_deref(), Some("feat/cli-feature"));
    assert_eq!(
        state.active_task.as_ref().map(|t| t.id.as_str()),
        Some("task-1")
    );
    assert_eq!(state.pending_tasks.len(), 1);
    assert_eq!(state.pending_tasks[0].id, "task-2");
}

#[test]
fn test_cli_milestones_mutually_exclusive_with_plan_and_goal() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "feat/cli-feature");

    let plan_file = repo_root.join("plan.md");
    fs::write(&plan_file, "### Task 1: Some task\n").unwrap();

    let json_milestones = r#"[{"title":"Step 1","verify_cmd":"true"}]"#;

    // milestones + plan
    let out_plan = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            json_milestones,
            "--plan",
            plan_file.to_str().unwrap(),
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run superstate");
    assert!(
        !out_plan.status.success(),
        "Should fail when combining --milestones and --plan"
    );

    // milestones + goal
    let out_goal = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            json_milestones,
            "--goal",
            "Direct goal",
            "--verify-cmd",
            "true",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run superstate");
    assert!(
        !out_goal.status.success(),
        "Should fail when combining --milestones and --goal"
    );
}

#[test]
fn test_cli_milestones_sequential_progression() {
    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().canonicalize().unwrap();
    init_git_repo(&repo_root, "feat/cli-feature");

    let json_milestones =
        r#"[{"title":"Step 1","verify_cmd":"true"},{"title":"Step 2","verify_cmd":"true"}]"#;
    let init_out = Command::new(bin())
        .args([
            "state",
            "init",
            "--milestones",
            json_milestones,
            "--worktree",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run superstate state init");
    assert!(init_out.status.success());

    let wt_task_1 = repo_root.join(".wt").join("feat-cli-feature-task-1");
    assert!(
        wt_task_1.exists(),
        ".wt/feat-cli-feature-task-1 must exist after init"
    );

    // Modify a file in .wt/feat-cli-feature-task-1 and commit it
    let file_in_wt1 = wt_task_1.join("step1.txt");
    fs::write(&file_in_wt1, b"completed step 1\n").unwrap();

    let add_status = Command::new("git")
        .args(["add", "step1.txt"])
        .current_dir(&wt_task_1)
        .status()
        .unwrap();
    assert!(add_status.success());

    let commit_status = Command::new("git")
        .args(["commit", "-m", "feat: step 1 done"])
        .current_dir(&wt_task_1)
        .status()
        .unwrap();
    assert!(commit_status.success());

    // Run state next
    let next_out = Command::new(bin())
        .args(["state", "next"])
        .current_dir(&repo_root)
        .output()
        .expect("Failed to run superstate state next");
    assert!(
        next_out.status.success(),
        "state next failed: stderr={}",
        String::from_utf8_lossy(&next_out.stderr)
    );

    // Asserts .wt/feat-cli-feature-task-1 was merged into feat/cli-feature and deleted
    assert!(
        !wt_task_1.exists(),
        ".wt/feat-cli-feature-task-1 must be deleted after merge"
    );
    let file_in_main = repo_root.join("step1.txt");
    assert!(
        file_in_main.exists(),
        "step1.txt must now exist on feat/cli-feature in repo root"
    );

    // Verify current branch in primary repo is still feat/cli-feature
    let branch_out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&branch_out.stdout).trim(),
        "feat/cli-feature"
    );

    // Asserts .wt/feat-cli-feature-task-2 was automatically provisioned, task-2 is active,
    // and state.active_worktree points to .wt/feat-cli-feature-task-2
    let wt_task_2 = repo_root.join(".wt").join("feat-cli-feature-task-2");
    assert!(
        wt_task_2.exists(),
        ".wt/feat-cli-feature-task-2 must be automatically provisioned"
    );

    let state_file = repo_root
        .join(".agent-state")
        .join("branches")
        .join("feat-cli-feature.json");
    let content = fs::read_to_string(&state_file).unwrap();
    let state: ExecutionState = serde_json::from_str(&content).unwrap();

    assert_eq!(state.step, 2);
    let active_task = state.active_task.expect("task-2 must be active");
    assert_eq!(active_task.id, "task-2");
    assert_eq!(active_task.title, "Step 2");
    assert_eq!(state.active_worktree, Some(wt_task_2));
    assert_eq!(
        state.active_branch,
        Some("wt/feat-cli-feature-task-2".to_string())
    );
    assert_eq!(state.completed_tasks.len(), 1);
    assert_eq!(state.completed_tasks[0].id, "task-1");
}
