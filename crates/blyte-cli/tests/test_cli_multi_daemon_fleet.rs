use blyte_cli::commands::daemon::{daemon_paths_for_state, is_pid_alive};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
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
fn test_daemon_paths_for_state() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();

    let default_state = state_dir.join("state.json");
    let default_paths = daemon_paths_for_state(&default_state);
    assert_eq!(default_paths.slug, "default");
    assert!(default_paths
        .pid_file
        .ends_with(".agent-state/daemons/default.pid"));
    assert!(default_paths
        .log_file
        .ends_with(".agent-state/logs/default.log"));
    assert!(default_paths
        .prompt_file
        .ends_with(".agent-state/prompts/default.md"));

    let branch_state = state_dir.join("branches").join("feat-alpha.json");
    let branch_paths = daemon_paths_for_state(&branch_state);
    assert_eq!(branch_paths.slug, "feat-alpha");
    assert!(branch_paths
        .pid_file
        .ends_with(".agent-state/daemons/feat-alpha.pid"));
    assert!(branch_paths
        .log_file
        .ends_with(".agent-state/logs/feat-alpha.log"));
    assert!(branch_paths
        .prompt_file
        .ends_with(".agent-state/prompts/feat-alpha.md"));
}

#[test]
fn test_cli_multi_daemon_fleet() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    // 1. Initialize branch state for feat/alpha
    let status_alpha = Command::new("git")
        .args(["checkout", "-b", "feat/alpha"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status_alpha.success());

    let init_alpha = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Alpha Goal",
            "--verify-cmd",
            "sleep 30 && true",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to init alpha state");
    assert!(
        init_alpha.status.success(),
        "Init alpha failed: {}",
        String::from_utf8_lossy(&init_alpha.stderr)
    );

    // 2. Initialize branch state for feat/beta
    let status_beta = Command::new("git")
        .args(["checkout", "-b", "feat/beta"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status_beta.success());

    let init_beta = Command::new(bin())
        .args([
            "state",
            "init",
            "--goal",
            "Beta Goal",
            "--verify-cmd",
            "sleep 30 && true",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to init beta state");
    assert!(
        init_beta.status.success(),
        "Init beta failed: {}",
        String::from_utf8_lossy(&init_beta.stderr)
    );

    let alpha_state = dir
        .path()
        .join(".agent-state")
        .join("branches")
        .join("feat-alpha.json");
    let beta_state = dir
        .path()
        .join(".agent-state")
        .join("branches")
        .join("feat-beta.json");
    assert!(alpha_state.exists());
    assert!(beta_state.exists());

    // 3. Spawn background daemon on feat/alpha
    let spawn_alpha = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            alpha_state.to_str().unwrap(),
            "--driver",
            "sleep 30",
            "--detach",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to spawn alpha daemon");
    assert!(
        spawn_alpha.status.success(),
        "Spawn alpha failed: {}",
        String::from_utf8_lossy(&spawn_alpha.stderr)
    );

    // 4. Spawn background daemon on feat/beta
    let spawn_beta = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            beta_state.to_str().unwrap(),
            "--driver",
            "sleep 30",
            "--detach",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to spawn beta daemon");
    assert!(
        spawn_beta.status.success(),
        "Spawn beta failed: {}",
        String::from_utf8_lossy(&spawn_beta.stderr)
    );

    // 5. Assert both have distinct PID files and log files in .agent-state/
    let alpha_pid_file = dir
        .path()
        .join(".agent-state")
        .join("daemons")
        .join("feat-alpha.pid");
    let beta_pid_file = dir
        .path()
        .join(".agent-state")
        .join("daemons")
        .join("feat-beta.pid");
    let alpha_log_file = dir
        .path()
        .join(".agent-state")
        .join("logs")
        .join("feat-alpha.log");
    let beta_log_file = dir
        .path()
        .join(".agent-state")
        .join("logs")
        .join("feat-beta.log");

    for _ in 0..60 {
        let alpha_ready = fs::read_to_string(&alpha_pid_file)
            .map(|s| s.trim().parse::<i32>().is_ok())
            .unwrap_or(false);
        let beta_ready = fs::read_to_string(&beta_pid_file)
            .map(|s| s.trim().parse::<i32>().is_ok())
            .unwrap_or(false);
        if alpha_ready && beta_ready && alpha_log_file.exists() && beta_log_file.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert!(alpha_pid_file.exists(), "alpha pid file should exist");
    assert!(beta_pid_file.exists(), "beta pid file should exist");
    assert!(alpha_log_file.exists(), "alpha log file should exist");
    assert!(beta_log_file.exists(), "beta log file should exist");

    let root_legacy_pid = dir.path().join(".agent-state").join("daemon.pid");
    let branches_legacy_pid = dir
        .path()
        .join(".agent-state")
        .join("branches")
        .join("daemon.pid");
    assert!(
        !root_legacy_pid.exists(),
        "root daemon.pid should not be written for branch daemons"
    );
    assert!(
        !branches_legacy_pid.exists(),
        "branches/daemon.pid should not be written for branch daemons"
    );

    let alpha_pid: i32 = fs::read_to_string(&alpha_pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let beta_pid: i32 = fs::read_to_string(&beta_pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    assert_ne!(alpha_pid, beta_pid, "PIDs must be distinct");
    assert!(is_pid_alive(alpha_pid), "Alpha daemon must be alive");
    assert!(is_pid_alive(beta_pid), "Beta daemon must be alive");

    // 6. Assert attempting to spawn a second daemon on feat/alpha fails with a concurrency error
    let dup_alpha = Command::new(bin())
        .args([
            "state",
            "daemon",
            "--state",
            alpha_state.to_str().unwrap(),
            "--driver",
            "sleep 30",
            "--detach",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run duplicate daemon spawn");
    assert!(
        !dup_alpha.status.success(),
        "Duplicate daemon spawn on feat/alpha should fail"
    );
    let dup_stderr = String::from_utf8_lossy(&dup_alpha.stderr);
    assert!(
        dup_stderr.contains("already running"),
        "Expected concurrency error message, got: {}",
        dup_stderr
    );

    // 7. Assert blyte state daemon list displays both active daemons
    let list_out = Command::new(bin())
        .args(["state", "daemon", "list"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run state daemon list");
    assert!(
        list_out.status.success(),
        "Daemon list failed: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        list_stdout.contains("feat-alpha"),
        "Daemon list output should contain feat-alpha: {}",
        list_stdout
    );
    assert!(
        list_stdout.contains("feat-beta"),
        "Daemon list output should contain feat-beta: {}",
        list_stdout
    );

    // 8. Assert blyte state cancel --all terminates both daemons cleanly
    let cancel_out = Command::new(bin())
        .args(["state", "cancel", "--all"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run state cancel --all");
    assert!(
        cancel_out.status.success(),
        "Cancel --all failed: {}",
        String::from_utf8_lossy(&cancel_out.stderr)
    );

    for _ in 0..40 {
        if !is_pid_alive(alpha_pid) && !is_pid_alive(beta_pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert!(
        !is_pid_alive(alpha_pid),
        "Alpha daemon should be terminated"
    );
    assert!(!is_pid_alive(beta_pid), "Beta daemon should be terminated");
    assert!(!alpha_pid_file.exists(), "Alpha PID file should be removed");
    assert!(!beta_pid_file.exists(), "Beta PID file should be removed");
}

#[test]
fn test_subdirectory_daemon_list() {
    let dir = tempdir().unwrap();
    init_git_repo(dir.path());

    let state_dir = dir.path().join(".agent-state");
    let daemons_dir = state_dir.join("daemons");
    fs::create_dir_all(&daemons_dir).unwrap();

    // Spawn dummy process
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id() as i32;
    fs::write(daemons_dir.join("default.pid"), pid.to_string()).unwrap();

    // Create deep nested subdirectory
    let sub = dir.path().join("crates").join("foo").join("src");
    fs::create_dir_all(&sub).unwrap();

    // Run `blyte daemon list` from subdirectory
    let output = Command::new(bin())
        .args(["daemon", "list"])
        .current_dir(&sub)
        .output()
        .expect("Failed to run daemon list from subdirectory");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {}", stderr);
    assert!(
        stdout.contains("default") && stdout.contains(&pid.to_string()),
        "Daemon list from subdirectory should find default daemon. Output: {}",
        stdout
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_cancel_branch_main_fallback() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".agent-state");
    let daemons_dir = state_dir.join("daemons");
    fs::create_dir_all(&daemons_dir).unwrap();
    let state_file = state_dir.join("state.json");
    fs::write(&state_file, "{}").unwrap();

    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id() as i32;
    let default_pid_file = daemons_dir.join("default.pid");
    fs::write(&default_pid_file, pid.to_string()).unwrap();

    // Cancel with branch "main" when main.pid doesn't exist
    let cancel_res =
        blyte_cli::commands::daemon::cmd_cancel(Some(&state_file), Some("main"), false);
    assert!(cancel_res.is_ok());
    assert!(
        !default_pid_file.exists(),
        "default.pid should be removed by branch main cancel fallback"
    );
    assert!(!is_pid_alive(pid), "Process should be killed");
    let _ = child.wait();
}

#[test]
fn test_cancel_all_deduplicates_pids() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".agent-state");
    let daemons_dir = state_dir.join("daemons");
    fs::create_dir_all(&daemons_dir).unwrap();
    let state_file = state_dir.join("state.json");
    fs::write(&state_file, "{}").unwrap();

    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id() as i32;
    let default_pid_file = daemons_dir.join("default.pid");
    let legacy_pid_file = state_dir.join("daemon.pid");
    fs::write(&default_pid_file, pid.to_string()).unwrap();
    fs::write(&legacy_pid_file, pid.to_string()).unwrap();

    // Cancel all
    let cancel_res = blyte_cli::commands::daemon::cmd_cancel(Some(&state_file), None, true);
    assert!(cancel_res.is_ok());
    assert!(!default_pid_file.exists(), "default.pid should be removed");
    assert!(!legacy_pid_file.exists(), "daemon.pid should be removed");
    assert!(!is_pid_alive(pid), "Process should be killed");
    let _ = child.wait();
}
