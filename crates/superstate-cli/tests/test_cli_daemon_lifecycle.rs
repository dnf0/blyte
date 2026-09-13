use std::fs;
use superstate_cli::commands::daemon::{cmd_cancel, cmd_daemon, cmd_logs, is_pid_alive};
use superstate_cli::commands::state::cmd_status;
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
        },
        MilestoneSpec {
            title: "Task 2".to_string(),
            verify_cmd: "sleep 0.2 && true".to_string(),
            files: vec![],
        },
    ];

    let state = init_state_milestones(Some("Daemon Goal"), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    // Start daemon detached
    cmd_daemon(
        &state_path,
        Some("echo 'working'"),
        true, // detach
        3,
        10,
    )
    .unwrap();

    // Allow spawned background process to start and flush initial banner
    let log_file = state_dir.join("logs").join("daemon.log");
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if !fs::read_to_string(&log_file).unwrap_or_default().is_empty() {
            break;
        }
    }
    let pid_file = state_dir.join("daemon.pid");

    assert!(pid_file.exists());
    let pid_str = fs::read_to_string(&pid_file).unwrap();
    let pid: i32 = pid_str.trim().parse().unwrap();
    assert!(is_pid_alive(pid));

    // Verify cmd_status works during daemon run
    cmd_status(&state_path, false).unwrap();
    cmd_status(&state_path, true).unwrap();

    // Cancel daemon
    cmd_cancel(&state_path).unwrap();

    assert!(!is_pid_alive(pid));

    // Inspect logs
    let logs_output = cmd_logs(&state_path, Some(10)).unwrap();
    assert!(!logs_output.is_empty());
}
