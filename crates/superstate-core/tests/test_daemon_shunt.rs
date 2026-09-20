use std::fs;
use superstate_core::daemon::{DaemonConfig, DaemonRunner};
use superstate_core::models::{ExecutionState, TaskSpec};
use superstate_core::state_machine::save_state_atomic;
use tempfile::tempdir;

#[test]
fn test_daemon_config_default_shunt_driver_output() {
    let config = DaemonConfig::default();
    assert!(
        config.shunt_driver_output,
        "DaemonConfig::default().shunt_driver_output must default to true"
    );
}

#[test]
fn test_daemon_config_deserialize_defaults_shunt_driver_output_to_true() {
    let json_str = r#"{
        "state_path": ".agent-state/state.json",
        "driver_cmd": null,
        "max_retries": 3,
        "timeout_secs": 120,
        "advisory_gate": false,
        "advisory_review": false,
        "review_blocking": false
    }"#;
    let config: DaemonConfig = serde_json::from_str(json_str).expect("deserialize config");
    assert!(
        config.shunt_driver_output,
        "deserialized DaemonConfig without shunt_driver_output field must default to true"
    );
}

#[test]
fn test_daemon_runner_injects_shunt_wrapper() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Test Shunt".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f driver_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test tool output shunting integration".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Driver command verifies SUPERSTATE_SHUNT_ENABLED=1 and that shunt is in PATH
    let driver_cmd = "test \"$SUPERSTATE_SHUNT_ENABLED\" = \"1\" && which shunt && echo 'success' > driver_evidence.txt";

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some(driver_cmd.to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: true,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    // Verify .agent-state/bin/shunt was created and is executable
    let shunt_script = dir.path().join(".agent-state/bin/shunt");
    assert!(
        shunt_script.exists(),
        "shunt wrapper script must exist in .agent-state/bin"
    );
    let content = fs::read_to_string(&shunt_script).unwrap();
    assert!(
        content.contains("#!/bin/sh") && content.contains("exec superstate shunt -- \"$@\""),
        "shunt wrapper script must invoke superstate shunt"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&shunt_script).unwrap().permissions();
        assert_eq!(
            perms.mode() & 0o777,
            0o755,
            "shunt script permissions must be 0o755"
        );
    }
}

#[test]
fn test_daemon_runner_no_shunt_disables_injection() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-2".to_string(),
            title: "Test No Shunt".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f no_shunt_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test disabling shunt integration".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    // Driver command verifies SUPERSTATE_SHUNT_ENABLED is empty/unset
    let driver_cmd =
        "test -z \"$SUPERSTATE_SHUNT_ENABLED\" && echo 'no_shunt_ok' > no_shunt_evidence.txt";

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some(driver_cmd.to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        guard_file_reads: false,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let shunt_script = dir.path().join(".agent-state/bin/shunt");
    assert!(
        !shunt_script.exists(),
        "shunt script must not be created when shunt_driver_output is false"
    );
}
