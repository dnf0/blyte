//! Turnkey prompt generation: task selection and ledger replay.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

fn setup(dir: &std::path::Path) -> std::path::PathBuf {
    let plan = dir.join("plan.md");
    std::fs::write(
        &plan,
        "### Task 1: Core abstractions\n- **Depends on:** none\n\
         ### Task 2: Backend adapter\n- **Depends on:** Task 1\n",
    )
    .unwrap();
    let state = dir.join("state.json");
    Command::new(bin())
        .args(["state", "init", "--plan"])
        .arg(&plan)
        .arg("--state")
        .arg(&state)
        .output()
        .unwrap();
    state
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn prompt_can_target_a_specific_task_not_only_the_active_one() {
    // Without --task, `state ready` is a query with nowhere to go: a runner can
    // learn which tasks are unblocked but cannot ask for a prompt for any of them.
    let dir = tempfile::tempdir().unwrap();
    let state = setup(dir.path());

    let (ok, stdout, _) = run(&[
        "state",
        "prompt",
        "--state",
        state.to_str().unwrap(),
        "--task",
        "Task 2: Backend adapter",
    ]);
    assert!(ok);
    assert!(stdout.contains("Task 2"), "{}", stdout);
    assert!(
        !stdout.contains("**Task 1:"),
        "wrong task selected: {}",
        stdout
    );
}

#[test]
fn an_unknown_task_is_an_error_not_a_silent_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let state = setup(dir.path());

    let (ok, _, stderr) = run(&[
        "state",
        "prompt",
        "--state",
        state.to_str().unwrap(),
        "--task",
        "Task 99",
    ]);
    assert!(!ok, "selecting a nonexistent task must fail");
    assert!(stderr.contains("No task matching"), "{}", stderr);
}

#[test]
fn the_prompt_replays_the_epistemic_ledger() {
    // `state note` records findings so later milestones inherit them. If the
    // prompt does not read them back, notes are written to something nothing
    // reads and each milestone starts blind.
    let dir = tempfile::tempdir().unwrap();
    let state = setup(dir.path());

    run(&[
        "state",
        "note",
        "Parser needs a lookahead token",
        "--state",
        state.to_str().unwrap(),
    ]);
    let (ok, stdout, _) = run(&["state", "prompt", "--state", state.to_str().unwrap()]);

    assert!(ok);
    assert!(stdout.contains("Epistemic Ledger"), "{}", stdout);
    assert!(stdout.contains("lookahead token"), "{}", stdout);
}

#[test]
fn with_graph_warns_rather_than_silently_doing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = setup(dir.path());

    let (ok, _, stderr) = run(&[
        "state",
        "prompt",
        "--state",
        state.to_str().unwrap(),
        "--with-graph",
    ]);
    assert!(ok, "a missing graph is not a failure");
    assert!(
        stderr.contains("--with-graph"),
        "an accepted flag that does nothing must say so: {}",
        stderr
    );
}

#[test]
fn superstate_exposes_state_commands() {
    for cmd in ["ready", "prompt", "note", "next", "status", "init"] {
        let out = Command::new(env!("CARGO_BIN_EXE_superstate"))
            .args(["state", cmd, "--help"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "`superstate state {}` missing",
            cmd
        );
    }
}

#[test]
fn prompt_compacts_overly_long_ledger_entries() {
    let dir = tempfile::tempdir().unwrap();
    let state = setup(dir.path());

    let long_note = format!(
        "### Summary of findings\n\n{}\n\nExcessive detail that should not bloat the prompt.",
        "A".repeat(500)
    );

    run(&[
        "state",
        "note",
        &long_note,
        "--state",
        state.to_str().unwrap(),
    ]);

    let (ok, stdout, _) = run(&["state", "prompt", "--state", state.to_str().unwrap()]);
    assert!(ok);
    assert!(stdout.contains("Epistemic Ledger"), "{}", stdout);
    // The prompt should NOT contain the full 500-char repeat or the trailing excessive detail
    assert!(
        !stdout.contains("Excessive detail that should not bloat the prompt"),
        "{}",
        stdout
    );
}

#[test]
fn prompt_includes_minimal_direct_fix_and_hard_stop_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let state = setup(dir.path());

    let (ok, stdout, _) = run(&["state", "prompt", "--state", state.to_str().unwrap()]);
    assert!(ok);
    assert!(
        stdout.contains("minimal direct fix"),
        "Prompt should instruct minimal direct fix: {}",
        stdout
    );
    assert!(
        stdout.contains("HARD STOP UPON PASSING"),
        "Prompt should instruct hard stop upon passing: {}",
        stdout
    );
}
