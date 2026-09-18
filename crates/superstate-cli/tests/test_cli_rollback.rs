use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use superstate_cli::state_cli::StateCommands;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

#[derive(Parser)]
struct StateWrapper {
    #[command(subcommand)]
    cmd: StateCommands,
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

    fs::write(path.join("tracked.txt"), "initial baseline\n").expect("write tracked.txt");
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
fn test_cli_state_daemon_parses_auto_rollback_flag() {
    let parsed = StateWrapper::try_parse_from(["state", "daemon", "--auto-rollback"])
        .expect("parse state daemon --auto-rollback");
    match parsed.cmd {
        StateCommands::Daemon(args) => {
            assert!(
                args.auto_rollback,
                "args.auto_rollback must be true when --auto-rollback is supplied"
            );
        }
        _ => panic!("Expected StateCommands::Daemon"),
    }
}

#[test]
fn test_cli_state_daemon_auto_rollback_default_false() {
    let parsed = StateWrapper::try_parse_from(["state", "daemon"]).expect("parse state daemon");
    match parsed.cmd {
        StateCommands::Daemon(args) => {
            assert!(
                !args.auto_rollback,
                "args.auto_rollback must be false by default"
            );
        }
        _ => panic!("Expected StateCommands::Daemon"),
    }
}

#[test]
fn test_cli_daemon_help_has_auto_rollback() {
    // Check `superstate daemon --help`
    let output = Command::new(bin())
        .args(["daemon", "--help"])
        .output()
        .expect("run superstate daemon --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--auto-rollback"),
        "`superstate daemon --help` should mention --auto-rollback, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Automatically roll back working tree to milestone baseline on verification failure"),
        "`superstate daemon --help` should describe --auto-rollback, got:\n{}",
        stdout
    );

    // Also check `superstate state daemon --help`
    let output_state = Command::new(bin())
        .args(["state", "daemon", "--help"])
        .output()
        .expect("run superstate state daemon --help");
    assert!(output_state.status.success());
    let stdout_state = String::from_utf8_lossy(&output_state.stdout);
    assert!(
        stdout_state.contains("--auto-rollback"),
        "`superstate state daemon --help` should mention --auto-rollback, got:\n{}",
        stdout_state
    );
    assert!(
        stdout_state.contains("Automatically roll back working tree to milestone baseline on verification failure"),
        "`superstate state daemon --help` should describe --auto-rollback, got:\n{}",
        stdout_state
    );
}

#[test]
fn test_cli_daemon_auto_rollback_execution() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    let state_dir = repo.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![superstate_core::models::MilestoneSpec {
        title: "Test Task".to_string(),
        verify_cmd: "test -f must_exist.txt".to_string(),
        files: vec![PathBuf::from("tracked.txt")],
    }];
    let state = superstate_core::state_machine::init_state_milestones(
        Some("Test Goal"),
        milestones,
        None,
        true,
    )
    .unwrap();
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    let mock_driver = "sh -c \"echo corrupted > tracked.txt && echo junk > untracked.txt\"";

    let output = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            state_path.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--max-retries",
            "1",
            "--auto-rollback",
        ])
        .current_dir(repo)
        .output()
        .expect("run state daemon with --auto-rollback");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stdout:\n{}\nstderr:\n{}", stdout, stderr);

    // Tracked file must be restored
    let tracked_content = fs::read_to_string(repo.join("tracked.txt")).unwrap();
    assert_eq!(
        tracked_content, "initial baseline\n",
        "tracked file should be rolled back to initial commit baseline"
    );

    // Untracked file must be removed by clean -fd
    assert!(
        !repo.join("untracked.txt").exists(),
        "untracked.txt should be removed by auto_rollback"
    );

    // Notes must contain [rollback]
    let updated_state = superstate_core::state_machine::load_state(&state_path).unwrap();
    let has_rollback_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[rollback]"));
    assert!(
        has_rollback_note,
        "state.notes should contain a [rollback] note"
    );
}

#[test]
fn test_cli_daemon_auto_rollback_disabled_by_default() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    let state_dir = repo.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![superstate_core::models::MilestoneSpec {
        title: "Test Task".to_string(),
        verify_cmd: "test -f must_exist.txt".to_string(),
        files: vec![PathBuf::from("tracked.txt")],
    }];
    let state = superstate_core::state_machine::init_state_milestones(
        Some("Test Goal"),
        milestones,
        None,
        true,
    )
    .unwrap();
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    let mock_driver = "sh -c \"echo corrupted > tracked.txt && echo junk > untracked.txt\"";

    let output = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            state_path.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--max-retries",
            "1",
        ])
        .current_dir(repo)
        .env_remove("SUPERSTATE_AUTO_ROLLBACK")
        .output()
        .expect("run state daemon without --auto-rollback");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stdout:\n{}\nstderr:\n{}", stdout, stderr);

    // Without auto_rollback, corrupted changes remain
    let tracked_content = fs::read_to_string(repo.join("tracked.txt")).unwrap();
    assert_eq!(
        tracked_content.trim(),
        "corrupted",
        "tracked file should NOT be rolled back when auto_rollback is false"
    );

    assert!(
        repo.join("untracked.txt").exists(),
        "untracked.txt should NOT be removed when auto_rollback is false"
    );

    let updated_state = superstate_core::state_machine::load_state(&state_path).unwrap();
    let has_rollback_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[rollback]"));
    assert!(
        !has_rollback_note,
        "state.notes should NOT contain a [rollback] note when auto_rollback is false"
    );
}

#[test]
fn test_cli_daemon_auto_rollback_via_env_var() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    let state_dir = repo.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![superstate_core::models::MilestoneSpec {
        title: "Test Task".to_string(),
        verify_cmd: "test -f must_exist.txt".to_string(),
        files: vec![PathBuf::from("tracked.txt")],
    }];
    let state = superstate_core::state_machine::init_state_milestones(
        Some("Test Goal"),
        milestones,
        None,
        true,
    )
    .unwrap();
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    let mock_driver = "sh -c \"echo corrupted > tracked.txt && echo junk > untracked.txt\"";

    // Run without --auto-rollback flag, but with SUPERSTATE_AUTO_ROLLBACK=1
    let output = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            state_path.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--max-retries",
            "1",
        ])
        .current_dir(repo)
        .env("SUPERSTATE_AUTO_ROLLBACK", "1")
        .output()
        .expect("run state daemon with SUPERSTATE_AUTO_ROLLBACK=1");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stdout:\n{}\nstderr:\n{}", stdout, stderr);

    // Tracked file must be restored
    let tracked_content = fs::read_to_string(repo.join("tracked.txt")).unwrap();
    assert_eq!(
        tracked_content, "initial baseline\n",
        "tracked file should be rolled back via SUPERSTATE_AUTO_ROLLBACK=1"
    );

    assert!(
        !repo.join("untracked.txt").exists(),
        "untracked.txt should be removed via SUPERSTATE_AUTO_ROLLBACK=1"
    );

    let updated_state = superstate_core::state_machine::load_state(&state_path).unwrap();
    let has_rollback_note = updated_state
        .notes
        .iter()
        .any(|n| n.note.contains("[rollback]"));
    assert!(
        has_rollback_note,
        "state.notes should contain a [rollback] note"
    );
}

#[test]
fn test_cli_daemon_detached_forwarding() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    let state_dir = repo.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![superstate_core::models::MilestoneSpec {
        title: "Test Task".to_string(),
        verify_cmd: "test -f must_exist.txt".to_string(),
        files: vec![PathBuf::from("tracked.txt")],
    }];
    let state = superstate_core::state_machine::init_state_milestones(
        Some("Test Goal"),
        milestones,
        None,
        true,
    )
    .unwrap();
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    let mock_driver = "sh -c \"echo corrupted > tracked.txt && echo junk > untracked.txt\"";

    // Spawn detached daemon with --auto-rollback
    let output = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            state_path.to_str().unwrap(),
            "--driver",
            mock_driver,
            "--max-retries",
            "1",
            "--auto-rollback",
            "--detach",
        ])
        .current_dir(repo)
        .env_remove("SUPERSTATE_AUTO_ROLLBACK")
        .output()
        .expect("run state daemon detached with --auto-rollback");

    assert!(output.status.success());

    // Read the PID written by the detached command and wait for background execution to complete
    let paths = superstate_core::daemon::daemon_paths_for_state(&state_path);
    assert!(paths.pid_file.exists(), "pid_file should exist after detach");
    let pid: i32 = fs::read_to_string(&paths.pid_file)
        .expect("read pid file")
        .trim()
        .parse()
        .expect("parse pid");

    for _ in 0..100 {
        if !superstate_core::daemon::is_pid_alive(pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        !superstate_core::daemon::is_pid_alive(pid),
        "background daemon should have finished within 5s"
    );

    let log_content = fs::read_to_string(&paths.log_file).unwrap_or_default();
    println!("Detached daemon log:\n{}", log_content);
    let updated_state = superstate_core::state_machine::load_state(&state_path).unwrap();
    println!("Detached state:\n{:?}", updated_state);

    // Verify that background daemon rolled back the corrupted changes
    let tracked_content = fs::read_to_string(repo.join("tracked.txt")).unwrap();
    assert_eq!(
        tracked_content, "initial baseline\n",
        "detached daemon should have received --auto-rollback and rolled back working tree"
    );
    assert!(
        !repo.join("untracked.txt").exists(),
        "detached daemon should have removed untracked.txt via --auto-rollback"
    );
}
