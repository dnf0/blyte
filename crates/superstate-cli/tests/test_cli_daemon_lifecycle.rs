use std::fs;
use superstate_cli::commands::daemon::{
    cmd_cancel, cmd_daemon, cmd_logs, daemon_paths_for_state, is_pid_alive,
};
use superstate_core::models::MilestoneSpec;
use superstate_core::state_machine::{init_state_milestones, save_state_atomic};
use tempfile::tempdir;

#[test]
fn test_cli_daemon_lifecycle() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![
        MilestoneSpec {
            title: "Task 1".to_string(),
            verify_cmd: "sleep 0.2 && true".to_string(),
            files: vec![],
            ..Default::default()
        },
        MilestoneSpec {
            title: "Task 2".to_string(),
            verify_cmd: "sleep 0.2 && true".to_string(),
            files: vec![],
            ..Default::default()
        },
    ];

    let state = init_state_milestones(Some("Daemon Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let paths = daemon_paths_for_state(&state_path);

    // Start daemon detached with long-running driver to test active cancellation
    cmd_daemon(
        &state_path,
        Some("sleep 30"),
        true, // detach
        3,
        10,
        false,
        false,
        false,
        true,
        false,
    )
    .unwrap();

    // Allow spawned background process to start and flush initial banner
    let log_file = paths.log_file;
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if !fs::read_to_string(&log_file).unwrap_or_default().is_empty() {
            break;
        }
    }
    let pid_file = paths.pid_file;

    assert!(pid_file.exists());
    let pid_str = fs::read_to_string(&pid_file).unwrap();
    let pid: i32 = pid_str.trim().parse().unwrap();
    assert!(is_pid_alive(pid));

    let legacy_pid = state_dir.join("daemon.pid");
    assert!(
        legacy_pid.exists(),
        "daemon.pid must exist for detached default daemon"
    );

    // Cancel daemon
    cmd_cancel(Some(&state_path), None, false).unwrap();

    assert!(!is_pid_alive(pid));
    assert!(
        !legacy_pid.exists(),
        "daemon.pid must be cleaned up on cancel"
    );

    // Inspect logs
    let logs_output = cmd_logs(Some(&state_path), None, Some(10)).unwrap();
    assert!(!logs_output.is_empty());
}

#[test]
fn test_cli_daemon_completion_cleans_legacy_pid() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let milestones = vec![MilestoneSpec {
        title: "Fast Task".to_string(),
        verify_cmd: "true".to_string(),
        files: vec![],
        ..Default::default()
    }];

    let state = init_state_milestones(Some("Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    let legacy_pid = state_dir.join("daemon.pid");
    fs::write(&legacy_pid, "99999").unwrap();

    // Run daemon to completion synchronously
    cmd_daemon(
        &state_path,
        None,
        false,
        1,
        5,
        false,
        false,
        false,
        true,
        false,
    )
    .unwrap();

    assert!(
        !legacy_pid.exists(),
        "daemon.pid must be removed on daemon completion"
    );
}

#[test]
fn test_kill_pid_terminates_process() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    assert!(is_pid_alive(pid));

    let res = superstate_cli::commands::daemon::kill_pid(pid);
    assert!(res.is_ok());
    assert!(!is_pid_alive(pid));
    let _ = child.wait();
}
