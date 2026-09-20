use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use superstate_core::command_guard::{
    evaluate_command_guard, CommandGuardConfig, CommandGuardDecision,
};
use tempfile::tempdir;

#[test]
fn test_config_default() {
    let config = CommandGuardConfig::default();
    assert_eq!(config.debounce_sec, 30);
    assert_eq!(config.state_path, None);
}

#[test]
fn test_serde_command_guard_decision() {
    let allow = CommandGuardDecision::Allow;
    let allow_json = serde_json::to_string(&allow).unwrap();
    assert_eq!(allow_json, r#"{"decision":"allow"}"#);
    let deserialized_allow: CommandGuardDecision = serde_json::from_str(&allow_json).unwrap();
    assert_eq!(deserialized_allow, allow);

    let deny = CommandGuardDecision::Deny {
        reason: "busy-wait blocked".to_string(),
    };
    let deny_json = serde_json::to_string(&deny).unwrap();
    assert_eq!(
        deny_json,
        r#"{"decision":"deny","reason":"busy-wait blocked"}"#
    );
    let deserialized_deny: CommandGuardDecision = serde_json::from_str(&deny_json).unwrap();
    assert_eq!(deserialized_deny, deny);
}

#[test]
fn test_unrelated_tool_allows_immediately() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file.clone()),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "path".to_string(),
        serde_json::Value::String("/foo/bar".to_string()),
    );

    let decision = evaluate_command_guard("view_file", &args, &config);
    assert_eq!(decision, CommandGuardDecision::Allow);
    // State file must not be created for unrelated tools
    assert!(!state_file.exists());
}

#[test]
fn test_manage_task_status_blocks_within_cooldown() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file.clone()),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "Action".to_string(),
        serde_json::Value::String("status".to_string()),
    );
    args.insert(
        "TaskId".to_string(),
        serde_json::Value::String("task-123".to_string()),
    );

    // First call: allowed
    let decision1 = evaluate_command_guard("manage_task", &args, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);
    assert!(state_file.exists());

    // Immediate second call: denied with expected reason format
    let decision2 = evaluate_command_guard("manage_task", &args, &config);
    match decision2 {
        CommandGuardDecision::Deny { reason } => {
            assert!(
                reason.contains("[superstate-guard] Busy-wait polling blocked:"),
                "unexpected reason: {}",
                reason
            );
            assert!(
                reason.contains("Cooldown active"),
                "reason missing cooldown: {}",
                reason
            );
            assert!(
                reason.contains("Do not loop on status; yield turn to await background task completion notification."),
                "reason missing guidance: {}",
                reason
            );
        }
        CommandGuardDecision::Allow => panic!("Expected Deny on immediate status poll"),
    }
}

#[test]
fn test_manage_task_list_shares_cooldown_with_status() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    let mut args_list = serde_json::Map::new();
    args_list.insert(
        "action".to_string(),
        serde_json::Value::String("list".to_string()),
    );

    let decision1 = evaluate_command_guard("manage_task", &args_list, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    let mut args_status = serde_json::Map::new();
    args_status.insert(
        "action".to_string(),
        serde_json::Value::String("status".to_string()),
    );

    let decision2 = evaluate_command_guard("manage_task", &args_status, &config);
    assert!(matches!(decision2, CommandGuardDecision::Deny { .. }));
}

#[test]
fn test_manage_task_non_polling_action_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    let mut args_kill = serde_json::Map::new();
    args_kill.insert(
        "Action".to_string(),
        serde_json::Value::String("kill".to_string()),
    );
    args_kill.insert(
        "TaskId".to_string(),
        serde_json::Value::String("task-1".to_string()),
    );

    let decision1 = evaluate_command_guard("manage_task", &args_kill, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    let decision2 = evaluate_command_guard("manage_task", &args_kill, &config);
    assert_eq!(decision2, CommandGuardDecision::Allow);

    let mut args_input = serde_json::Map::new();
    args_input.insert(
        "Action".to_string(),
        serde_json::Value::String("send_input".to_string()),
    );
    let decision3 = evaluate_command_guard("manage_task", &args_input, &config);
    assert_eq!(decision3, CommandGuardDecision::Allow);
}

#[test]
fn test_run_command_procfs_polling_blocks() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    let mut args1 = serde_json::Map::new();
    args1.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("cat /proc/1234/status".to_string()),
    );

    let decision1 = evaluate_command_guard("run_command", &args1, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    // Another procfs query on cmdline within debounce window should block
    let mut args2 = serde_json::Map::new();
    args2.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("cat /proc/1234/cmdline".to_string()),
    );

    let decision2 = evaluate_command_guard("run_command", &args2, &config);
    assert!(matches!(decision2, CommandGuardDecision::Deny { .. }));

    // Wildcard procfs query should also match pattern
    let mut args3 = serde_json::Map::new();
    args3.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("ls /proc/*/stat".to_string()),
    );
    let decision3 = evaluate_command_guard("run_command", &args3, &config);
    assert!(matches!(decision3, CommandGuardDecision::Deny { .. }));
}

#[test]
fn test_run_command_process_inspection_blocks() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    // ps aux
    let mut args1 = serde_json::Map::new();
    args1.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("ps aux | grep worker".to_string()),
    );
    let decision1 = evaluate_command_guard("run_command", &args1, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    // pgrep within cooldown should block
    let mut args2 = serde_json::Map::new();
    args2.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("pgrep -f worker".to_string()),
    );
    let decision2 = evaluate_command_guard("run_command", &args2, &config);
    assert!(matches!(decision2, CommandGuardDecision::Deny { .. }));

    // Non-matching words like 'stop_server' or 'laptop' should not trigger process inspection regex false-positives
    // Create new clean state
    let state_file_clean = dir.path().join("clean_history.json");
    let config_clean = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file_clean),
    };
    let mut args_benign = serde_json::Map::new();
    args_benign.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("./scripts/stop_server.sh".to_string()),
    );
    let decision_benign = evaluate_command_guard("run_command", &args_benign, &config_clean);
    assert_eq!(decision_benign, CommandGuardDecision::Allow);
}

#[test]
fn test_run_command_exact_duplicate_blocks() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("cargo test --test foo".to_string()),
    );

    let decision1 = evaluate_command_guard("run_command", &args, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    // Exact duplicate within cooldown blocks
    let decision2 = evaluate_command_guard("run_command", &args, &config);
    assert!(matches!(decision2, CommandGuardDecision::Deny { .. }));

    // Different command is allowed!
    let mut args_diff = serde_json::Map::new();
    args_diff.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("cargo check".to_string()),
    );
    let decision3 = evaluate_command_guard("run_command", &args_diff, &config);
    assert_eq!(decision3, CommandGuardDecision::Allow);
}

#[test]
fn test_run_command_exact_duplicate_whitespace_normalization() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    let mut args1 = serde_json::Map::new();
    args1.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("npm   run   build".to_string()),
    );
    let decision1 = evaluate_command_guard("run_command", &args1, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    let mut args2 = serde_json::Map::new();
    args2.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("  npm run build  ".to_string()),
    );
    let decision2 = evaluate_command_guard("run_command", &args2, &config);
    assert!(matches!(decision2, CommandGuardDecision::Deny { .. }));
}

#[test]
fn test_cooldown_expiry_allows_subsequent_execution() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");

    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Pre-populate state file with an entry from 40 seconds ago (debounce is 30s)
    let mut history = BTreeMap::new();
    history.insert("manage_task_status".to_string(), now_sec - 40);
    fs::write(&state_file, serde_json::to_string(&history).unwrap()).unwrap();

    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file.clone()),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "Action".to_string(),
        serde_json::Value::String("status".to_string()),
    );

    // Call after expiry should be Allowed
    let decision = evaluate_command_guard("manage_task", &args, &config);
    assert_eq!(decision, CommandGuardDecision::Allow);

    // Timestamp should be refreshed
    let content = fs::read_to_string(&state_file).unwrap();
    let updated: BTreeMap<String, u64> = serde_json::from_str(&content).unwrap();
    assert!(updated.get("manage_task_status").unwrap() >= &now_sec);
}

#[test]
fn test_prunes_entries_older_than_twice_debounce() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");

    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Setup history with:
    // - "recent": 10 seconds ago (should keep)
    // - "stale": 70 seconds ago (> 2 * 30s = 60s, should be pruned)
    let mut history = BTreeMap::new();
    history.insert("recent_cmd".to_string(), now_sec - 10);
    history.insert("stale_cmd".to_string(), now_sec - 70);
    fs::write(&state_file, serde_json::to_string(&history).unwrap()).unwrap();

    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file.clone()),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "CommandLine".to_string(),
        serde_json::Value::String("cargo build".to_string()),
    );

    let decision = evaluate_command_guard("run_command", &args, &config);
    assert_eq!(decision, CommandGuardDecision::Allow);

    let content = fs::read_to_string(&state_file).unwrap();
    let updated: BTreeMap<String, u64> = serde_json::from_str(&content).unwrap();
    assert!(updated.contains_key("recent_cmd"));
    assert!(!updated.contains_key("stale_cmd"));
    assert!(updated.contains_key("cmd:cargo build"));
}

#[test]
fn test_fail_open_on_corrupted_state_file() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");

    // Write invalid JSON
    fs::write(&state_file, "{ corrupted json syntax !@#").unwrap();

    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file.clone()),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "Action".to_string(),
        serde_json::Value::String("status".to_string()),
    );

    // Fail-open: returns Allow, does not panic, and rewrites clean JSON
    let decision = evaluate_command_guard("manage_task", &args, &config);
    assert_eq!(decision, CommandGuardDecision::Allow);

    let content = fs::read_to_string(&state_file).unwrap();
    let parsed: Result<BTreeMap<String, u64>, _> = serde_json::from_str(&content);
    assert!(parsed.is_ok(), "State file should be overwritten with valid JSON");
}

#[test]
fn test_fail_open_on_unwritable_directory() {
    let dir = tempdir().unwrap();
    // Create a regular file and try to place state file as a child of it
    let blocking_file = dir.path().join("blocking_file");
    fs::write(&blocking_file, "blocking").unwrap();
    let invalid_state_file = blocking_file.join("sub").join("guard_history.json");

    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(invalid_state_file),
    };

    let mut args = serde_json::Map::new();
    args.insert(
        "Action".to_string(),
        serde_json::Value::String("status".to_string()),
    );

    // Should return Allow without panicking
    let decision = evaluate_command_guard("manage_task", &args, &config);
    assert_eq!(decision, CommandGuardDecision::Allow);
}

#[test]
fn test_case_insensitivity_of_tool_and_arg_keys() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let config = CommandGuardConfig {
        debounce_sec: 30,
        state_path: Some(state_file),
    };

    // Tool name in uppercase "RUN_COMMAND" and argument "command"
    let mut args1 = serde_json::Map::new();
    args1.insert(
        "command".to_string(),
        serde_json::Value::String("ps -ef".to_string()),
    );
    let decision1 = evaluate_command_guard("RUN_COMMAND", &args1, &config);
    assert_eq!(decision1, CommandGuardDecision::Allow);

    // Immediate repeat with "cmd"
    let mut args2 = serde_json::Map::new();
    args2.insert(
        "cmd".to_string(),
        serde_json::Value::String("ps   aux".to_string()),
    );
    let decision2 = evaluate_command_guard("run_command", &args2, &config);
    assert!(matches!(decision2, CommandGuardDecision::Deny { .. }));
}
