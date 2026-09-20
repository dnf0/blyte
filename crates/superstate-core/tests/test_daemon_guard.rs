use std::fs;
use superstate_core::daemon::{DaemonConfig, DaemonRunner};
use superstate_core::models::{ExecutionState, TaskSpec};
use superstate_core::state_machine::save_state_atomic;
use tempfile::tempdir;

#[test]
fn test_daemon_config_default_guard_file_reads() {
    let config = DaemonConfig::default();
    assert!(
        config.guard_file_reads,
        "DaemonConfig::default().guard_file_reads must default to true"
    );
}

#[test]
fn test_daemon_config_deserialize_defaults_guard_file_reads_to_true() {
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
        config.guard_file_reads,
        "deserialized DaemonConfig without guard_file_reads field must default to true"
    );
}

#[test]
fn test_daemon_runner_provisions_hooks_json() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Test Guard".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f driver_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test automated workspace hook provisioning".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'driver ran' > driver_evidence.txt";

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
        guard_file_reads: true,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(
        hooks_file.exists(),
        ".agents/hooks.json must be provisioned when guard_file_reads is true"
    );

    let content = fs::read_to_string(&hooks_file).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).expect("valid JSON in hooks.json");

    let hook_entry = &json["superstate-file-guard"]["PreToolUse"][0];
    assert_eq!(
        hook_entry["matcher"].as_str(),
        Some("view_file|View|read_file"),
        "matcher must match view_file|View|read_file"
    );
    assert_eq!(
        hook_entry["hooks"][0]["command"].as_str(),
        Some("superstate guard pre-tool-use --threshold-lines 200"),
        "hook command must be superstate guard pre-tool-use --threshold-lines 200"
    );
}

#[test]
fn test_daemon_runner_no_guard_does_not_provision_hooks_json() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-2".to_string(),
            title: "Test No Guard".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f no_guard_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test disabling guard hook provisioning".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'no guard ran' > no_guard_evidence.txt";

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
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(
        !hooks_file.exists(),
        ".agents/hooks.json must not be provisioned when guard_file_reads is false"
    );
}

#[test]
fn test_daemon_runner_preserves_existing_hooks_json() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let agents_dir = dir.path().join(".agents");
    fs::create_dir_all(&agents_dir).unwrap();
    let hooks_file = agents_dir.join("hooks.json");
    let custom_json = r#"{"custom_hook": true}"#;
    fs::write(&hooks_file, custom_json).unwrap();

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-3".to_string(),
            title: "Test Preserve Hooks".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f preserve_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test preserving existing hooks.json".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'preserve ran' > preserve_evidence.txt";

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
        guard_file_reads: true,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let content = fs::read_to_string(&hooks_file).unwrap();
    assert_eq!(
        content, custom_json,
        "existing .agents/hooks.json must not be overwritten"
    );
}
