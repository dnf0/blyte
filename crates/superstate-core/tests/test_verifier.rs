use std::fs;
use std::process::Command;
use superstate_core::models::TaskSpec;
use superstate_core::verifier::{execute_all_gates, run_verification_cmd, verify_git_clean};
use tempfile::tempdir;

#[test]
fn test_verification_command_success_and_failure() {
    let dir = tempdir().unwrap();
    let ok_res = run_verification_cmd("echo 'hello'", dir.path(), 5).unwrap();
    assert!(ok_res.passed);
    assert!(ok_res.output.contains("hello"));
    assert!(ok_res.failure_reason.is_none());

    let fail_res = run_verification_cmd("exit 1", dir.path(), 5).unwrap();
    assert!(!fail_res.passed);
    assert!(fail_res.failure_reason.is_some());
}

#[test]
fn test_verify_git_clean() {
    let dir = tempdir().unwrap();
    // Initialize git repo in dir
    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let clean_res = verify_git_clean(dir.path()).unwrap();
    assert!(clean_res.passed);

    // Create untracked dirty file
    fs::write(dir.path().join("dirty.txt"), "untracked").unwrap();
    let dirty_res = verify_git_clean(dir.path()).unwrap();
    assert!(!dirty_res.passed);
    assert!(dirty_res.failure_reason.is_some());
}

#[test]
fn test_execute_all_gates() {
    let dir = tempdir().unwrap();
    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let task = TaskSpec {
        id: "Task 1".to_string(),
        title: "Test Task".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        interfaces_produces: vec![],
        verification_cmd: Some("echo 'gate ok'".to_string()),
    };

    let gates = execute_all_gates(dir.path(), &task, true).unwrap();
    assert_eq!(gates.len(), 2);
    assert!(gates.iter().all(|g| g.passed));
}
