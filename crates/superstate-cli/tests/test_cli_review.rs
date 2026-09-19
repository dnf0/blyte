use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_core::ledger::read_ledger;
use superstate_core::models::{ExecutionState, ExecutionStatus};
use superstate_core::state_machine::save_state_atomic;
use superstate_review::schema::{ReviewReport, Severity};
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
        .args(["commit", "-m", "chore: initial baseline"])
        .current_dir(path)
        .output()
        .expect("git commit");
}

#[test]
fn test_review_empty_diff_text_and_json() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // 1. Text mode with empty diff
    let output = Command::new(bin())
        .args(["review", "--repo", repo.to_str().unwrap()])
        .output()
        .expect("run superstate review");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No changes detected in diff. Review clean."));

    // 2. JSON mode with empty diff
    let output_json = Command::new(bin())
        .args(["review", "--repo", repo.to_str().unwrap(), "--json"])
        .output()
        .expect("run superstate review --json");
    assert!(output_json.status.success());
    let stdout_json = String::from_utf8_lossy(&output_json.stdout);
    let report: ReviewReport = serde_json::from_str(&stdout_json).expect("valid ReviewReport JSON");
    assert!(!report.has_blocking_issues);
    assert!(report.findings.is_empty());
    assert!(report.summary.contains("No changes detected in diff."));
}

#[test]
fn test_review_mock_driver_with_json() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Make an uncommitted change
    fs::write(repo.join("README.md"), "modified content\n").unwrap();

    let mock_driver = "echo '{\"summary\":\"Found 1 advisory issue\",\"has_blocking_issues\":false,\"findings\":[{\"severity\":\"Low\",\"file\":\"README.md\",\"line_range\":[1,1],\"problem\":\"Missing trailing newline\",\"suggested_fix\":\"Add newline\"}]}'";

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
        .expect("run superstate review with mock driver");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report: ReviewReport = serde_json::from_str(&stdout).expect("valid JSON review report");
    assert_eq!(report.summary, "Found 1 advisory issue");
    assert!(!report.has_blocking_issues);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].severity, Severity::Low);
    assert_eq!(report.findings[0].problem, "Missing trailing newline");
}

#[test]
fn test_review_record_to_ledger() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Initialize state.json
    let state_dir = repo.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");
    let initial_state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 0,
        ..Default::default()
    };
    save_state_atomic(&state_path, &initial_state).unwrap();

    // Make a change and stage it
    fs::write(repo.join("lib.rs"), "pub fn dangerous() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output()
        .unwrap();

    let mock_driver = "echo '{\"summary\":\"Security advisory\",\"has_blocking_issues\":true,\"findings\":[{\"severity\":\"High\",\"file\":\"lib.rs\",\"line_range\":[1,1],\"problem\":\"Insecure API function\",\"suggested_fix\":\"Add authentication guard\"}]}'";

    let output = Command::new(bin())
        .args([
            "review",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--record",
        ])
        .output()
        .expect("run superstate review --record");

    assert!(
        output.status.success(),
        "Review without --fail-on-blocking should exit 0 even if blocking issues exist"
    );

    // Verify ledger has the note appended
    let entries = read_ledger(&state_path);
    assert_eq!(entries.len(), 1, "Ledger should contain 1 recorded note");
    assert_eq!(entries[0].author, "reviewer");
    assert_eq!(
        entries[0].note,
        "[review-advisory] lib.rs:1-1: Insecure API function (Fix: Add authentication guard)"
    );
}

#[test]
fn test_review_records_only_critical_and_high_findings_to_ledger() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    let state_path = repo.join(".agent-state/state.json");
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    let initial_state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 0,
        ..Default::default()
    };
    save_state_atomic(&state_path, &initial_state).unwrap();

    fs::write(repo.join("lib.rs"), "pub fn mixed() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output()
        .unwrap();

    let mock_driver = "echo '{\"summary\":\"Mixed severity advisory\",\"has_blocking_issues\":true,\"findings\":[{\"severity\":\"Low\",\"file\":\"lib.rs\",\"line_range\":[1,1],\"problem\":\"Minor nit\",\"suggested_fix\":\"Ignore\"},{\"severity\":\"Medium\",\"file\":\"lib.rs\",\"line_range\":[1,1],\"problem\":\"Medium concern\",\"suggested_fix\":\"Improve\"},{\"severity\":\"High\",\"file\":\"lib.rs\",\"line_range\":[1,1],\"problem\":\"High vulnerability\",\"suggested_fix\":\"Fix auth\"},{\"severity\":\"Critical\",\"file\":\"lib.rs\",\"line_range\":[1,1],\"problem\":\"Critical exploit\",\"suggested_fix\":\"Patch immediately\"}]}'";

    let output = Command::new(bin())
        .args([
            "review",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--record",
        ])
        .output()
        .expect("run superstate review --record");

    assert!(output.status.success());

    let entries = read_ledger(&state_path);
    assert_eq!(
        entries.len(),
        2,
        "Only Critical and High findings should be recorded"
    );
    assert!(entries
        .iter()
        .any(|e| e.note.contains("High vulnerability")));
    assert!(entries.iter().any(|e| e.note.contains("Critical exploit")));
    assert!(!entries.iter().any(|e| e.note.contains("Minor nit")));
    assert!(!entries.iter().any(|e| e.note.contains("Medium concern")));
}

#[test]
fn test_review_fail_on_blocking() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Make an unstaged change to tracked file
    fs::write(repo.join("README.md"), "fn main() { panic!(); }\n").unwrap();

    let mock_driver = "echo '{\"summary\":\"Blocking crash found\",\"has_blocking_issues\":true,\"findings\":[{\"severity\":\"Critical\",\"file\":\"README.md\",\"line_range\":[1,1],\"problem\":\"Explicit panic in main\",\"suggested_fix\":\"Handle error gracefully\"}]}'";

    // 1. With --fail-on-blocking, exit code must be non-zero
    let output = Command::new(bin())
        .args([
            "review",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--fail-on-blocking",
        ])
        .output()
        .expect("run superstate review --fail-on-blocking");

    assert!(
        !output.status.success(),
        "Expected command to fail on blocking issues"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Review failed: blocking issues detected"),
        "stderr should mention blocking issues detected, got: {}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Review Summary: Blocking crash found"),
        "stdout should still contain report output, got: {}",
        stdout
    );

    // 2. Without --fail-on-blocking, same report should succeed
    let output_ok = Command::new(bin())
        .args([
            "review",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
        ])
        .output()
        .expect("run superstate review without --fail-on-blocking");

    assert!(output_ok.status.success());
}

#[test]
fn test_review_explicit_commit_range() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Make a commit
    fs::write(repo.join("file1.txt"), "first addition\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "feat: file 1"])
        .current_dir(repo)
        .output()
        .unwrap();

    // Make a second commit
    fs::write(repo.join("file2.txt"), "second addition\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "feat: file 2"])
        .current_dir(repo)
        .output()
        .unwrap();

    let mock_driver = "echo '{\"summary\":\"Commit range review passed\",\"has_blocking_issues\":false,\"findings\":[]}'";

    // Review HEAD~1..HEAD (only file2.txt)
    let output = Command::new(bin())
        .args([
            "review",
            "HEAD~1..HEAD",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--json",
        ])
        .output()
        .expect("run superstate review with commit range");

    if !output.status.success() {
        panic!(
            "superstate review failed:\nstatus: {:?}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let report: ReviewReport =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(report.summary, "Commit range review passed");
}

#[test]
fn test_review_explicit_state_path() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Custom state path outside default location
    let custom_state_dir = tmp.path().join("custom_state_dir");
    fs::create_dir_all(&custom_state_dir).unwrap();
    let custom_state_path = custom_state_dir.join("my_state.json");
    let initial_state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        ..Default::default()
    };
    save_state_atomic(&custom_state_path, &initial_state).unwrap();

    fs::write(repo.join("README.md"), "change for review\n").unwrap();

    let mock_driver = "echo '{\"summary\":\"Review with custom state path\",\"has_blocking_issues\":true,\"findings\":[{\"severity\":\"High\",\"file\":\"README.md\",\"line_range\":[1,2],\"problem\":\"Custom state test\",\"suggested_fix\":\"None\"}]}'";

    let output = Command::new(bin())
        .args([
            "review",
            "--repo",
            repo.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--state-path",
            custom_state_path.to_str().unwrap(),
            "--record",
        ])
        .output()
        .expect("run superstate review with custom state path");

    assert!(output.status.success());
    let entries = read_ledger(&custom_state_path);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].note.contains("Custom state test"));
}
