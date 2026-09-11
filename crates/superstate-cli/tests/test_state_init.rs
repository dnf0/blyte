//! Tests for state init greenfield Git baseline and exclusion setup.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

#[test]
fn init_auto_initializes_git_in_greenfield_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("plan.md");
    std::fs::write(&plan, "### Task 1: Hello\n- Verification Command: true\n").unwrap();
    let state = dir.path().join(".agent-state").join("state.json");

    let out = Command::new(bin())
        .args(["state", "init", "--plan"])
        .arg(&plan)
        .arg("--state")
        .arg(&state)
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(out.status.success(), "init failed: {:?}", out);
    assert!(
        dir.path().join(".git").exists(),
        ".git should be auto-initialized"
    );

    // Working tree should be clean because .agent-state and plan.md should be excluded
    let status_out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(status_out.status.success());
    let porcelain = String::from_utf8_lossy(&status_out.stdout);
    assert!(
        porcelain.trim().is_empty(),
        "Working tree should be clean, got: {}",
        porcelain
    );
}
