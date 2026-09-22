use std::fs;
use superstate_core::daemon::{workspace_fingerprint, DaemonConfig, DaemonRunner};
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
fn test_daemon_config_default_guard_commands() {
    let config = DaemonConfig::default();
    assert!(
        config.guard_commands,
        "DaemonConfig::default().guard_commands must default to true"
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
fn test_daemon_config_deserialize_defaults_guard_commands_to_true() {
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
        config.guard_commands,
        "deserialized DaemonConfig without guard_commands field must default to true"
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
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: true,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(
        hooks_file.exists(),
        ".agents/hooks.json must be provisioned when guards are active"
    );

    let content = fs::read_to_string(&hooks_file).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).expect("valid JSON in hooks.json");

    let file_hook_entry = &json["superstate-file-guard"]["PreToolUse"][0];
    assert_eq!(
        file_hook_entry["matcher"].as_str(),
        Some("view_file|View|read_file"),
        "matcher must match view_file|View|read_file"
    );
    let cmd = file_hook_entry["hooks"][0]["command"].as_str();
    assert_eq!(
        // 640 is deliberately NOT the default: this asserts the hook carries the CONFIGURED
        // budget, so a sweep that varies `read_threshold_lines` actually reaches the guard.
        cmd.map(|c: &str| c.ends_with("guard pre-tool-use --threshold-lines 640 --read-cache true")),
        // The hook must invoke the RUNNING binary by absolute path. A bare name resolves
        // through PATH at hook time, so EXP-077 enforced its guards with the previous day's
        // build while the daemon ran the current one.
        Some(true),
        "hook command must carry BOTH the configured budget and the cache switch, got {cmd:?}"
    );

    let cmd_hook_entry = &json["superstate-command-guard"]["PreToolUse"][0];
    assert_eq!(
        cmd_hook_entry["matcher"].as_str(),
        Some("run_command|manage_task"),
        "matcher must match run_command|manage_task"
    );
    let cmd = cmd_hook_entry["hooks"][0]["command"].as_str();
    assert_eq!(
        cmd.map(|c: &str| c.ends_with("guard pre-command --debounce-sec 30")),
        // The hook must invoke the RUNNING binary by absolute path. A bare name resolves
        // through PATH at hook time, so EXP-077 enforced its guards with the previous day's
        // build while the daemon ran the current one.
        Some(true),
        "hook command must end with `guard pre-command --debounce-sec 30`, got {cmd:?}"
    );
}

#[test]
fn test_daemon_provisions_command_guard_in_hooks_json() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-cmd-guard".to_string(),
            title: "Test Command Guard".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f driver_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test automated command guard hook provisioning".to_string()),
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
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: true,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(
        hooks_file.exists(),
        ".agents/hooks.json must be provisioned when guard_commands is true"
    );

    let content = fs::read_to_string(&hooks_file).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).expect("valid JSON in hooks.json");

    let cmd_hook_entry = &json["superstate-command-guard"]["PreToolUse"][0];
    assert_eq!(
        cmd_hook_entry["matcher"].as_str(),
        Some("run_command|manage_task"),
        "matcher must match run_command|manage_task"
    );
    let cmd = cmd_hook_entry["hooks"][0]["command"].as_str();
    assert_eq!(
        cmd.map(|c: &str| c.ends_with("guard pre-command --debounce-sec 30")),
        // Absolute path, not a bare name: PATH resolution at hook time is what made EXP-077
        // enforce its guards with the previous day's build.
        Some(true),
        "hook command must end with `guard pre-command --debounce-sec 30`, got {cmd:?}"
    );
}

#[test]
fn test_daemon_no_command_guard_disables_provisioning() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-no-cmd".to_string(),
            title: "Test No Command Guard".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f no_cmd_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test disabling command guard provisioning".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'ran' > no_cmd_evidence.txt";

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some(driver_cmd.to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(hooks_file.exists());

    let content = fs::read_to_string(&hooks_file).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).expect("valid JSON in hooks.json");

    assert!(
        !json["superstate-file-guard"].is_null(),
        "superstate-file-guard must be present when guard_file_reads is true"
    );
    assert!(
        json["superstate-command-guard"].is_null(),
        "superstate-command-guard must NOT be present when guard_commands is false"
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
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: false,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(
        !hooks_file.exists(),
        ".agents/hooks.json must not be provisioned when both guard flags are false"
    );
}

#[test]
fn test_daemon_no_guard_disables_both() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-no-guards".to_string(),
            title: "Test No Guards".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f no_guards_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test disabling both guards".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'ran' > no_guards_evidence.txt";

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some(driver_cmd.to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: false,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: false,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let hooks_file = dir.path().join(".agents/hooks.json");
    assert!(
        !hooks_file.exists(),
        ".agents/hooks.json must not be provisioned when guard_file_reads and guard_commands are false"
    );
}

#[test]
fn test_daemon_merges_hooks_when_file_exists() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let agents_dir = dir.path().join(".agents");
    fs::create_dir_all(&agents_dir).unwrap();
    let hooks_file = agents_dir.join("hooks.json");
    let custom_json = r#"{"custom_hook": {"PreToolUse": [{"matcher": "foo", "hooks": []}]}}"#;
    fs::write(&hooks_file, custom_json).unwrap();

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-merge".to_string(),
            title: "Test Merge Hooks".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f merge_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test merging into existing hooks.json".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'ran' > merge_evidence.txt";

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some(driver_cmd.to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: true,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let content = fs::read_to_string(&hooks_file).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).expect("valid JSON in hooks.json");

    assert!(
        !json["custom_hook"].is_null(),
        "existing user hook must be preserved"
    );
    assert_eq!(
        json["custom_hook"]["PreToolUse"][0]["matcher"].as_str(),
        Some("foo")
    );
    assert!(
        !json["superstate-file-guard"].is_null(),
        "superstate-file-guard must be merged into existing hooks.json"
    );
    assert!(
        !json["superstate-command-guard"].is_null(),
        "superstate-command-guard must be merged into existing hooks.json"
    );
}

#[test]
fn test_workspace_fingerprint_excludes_agents_dir() {
    let dir = tempdir().unwrap();
    let base_file = dir.path().join("file.txt");
    fs::write(&base_file, "initial content").unwrap();

    let initial_fp = workspace_fingerprint(dir.path());

    let agents_dir = dir.path().join(".agents");
    fs::create_dir_all(&agents_dir).unwrap();
    let hooks_file = agents_dir.join("hooks.json");
    fs::write(&hooks_file, r#"{"superstate-file-guard": {}}"#).unwrap();

    let after_fp = workspace_fingerprint(dir.path());
    assert_eq!(
        initial_fp, after_fp,
        "workspace_fingerprint must not change when .agents/hooks.json is added"
    );
}

#[test]
fn test_daemon_runner_provisions_hooks_json_appends_to_git_exclude() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");
    let git_info_dir = dir.path().join(".git/info");
    fs::create_dir_all(&git_info_dir).unwrap();
    let exclude_file = git_info_dir.join("exclude");
    fs::write(&exclude_file, "# git exclude\n*.tmp\n").unwrap();

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-exclude".to_string(),
            title: "Test Git Exclude".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f exclude_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Test git exclude provisioning".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let driver_cmd = "echo 'exclude ran' > exclude_evidence.txt";
    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some(driver_cmd.to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: true,
        impact_slicing: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());

    let exclude_content = fs::read_to_string(&exclude_file).unwrap();
    assert!(
        exclude_content.lines().any(|line| line.trim() == ".agents/" || line.trim() == ".agents"),
        ".git/info/exclude must contain .agents/ after provisioning"
    );
}


#[test]
fn generated_hooks_invoke_the_running_binary_not_whatever_is_on_path() {
    // A bare `superstate guard ...` resolves through PATH at hook time, so a run pinned to a
    // freshly built binary still executed whatever happened to be installed. EXP-077 ran the
    // current build as the daemon and the previous day's build as its guards: every fix under
    // test was absent from the process actually enforcing them, and the run reported zero
    // denials because that binary had no denial log to write.
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    std::fs::create_dir_all(repo.join(".agent-state")).expect("state dir");

    let state_path = repo.join(".agent-state").join("state.json");
    superstate_core::state_machine::save_state_atomic(
        &state_path,
        &superstate_core::models::ExecutionState {
            status: superstate_core::models::ExecutionStatus::InProgress,
            step: 1,
            active_task: Some(superstate_core::models::TaskSpec {
                id: "task-hooks".to_string(),
                title: "Hook provisioning".to_string(),
                verification_cmd: Some("true".to_string()),
                ..Default::default()
            }),
            no_worktree: true,
            ..Default::default()
        },
    )
    .expect("save state");

    let mut runner = superstate_core::daemon::DaemonRunner::new(superstate_core::daemon::DaemonConfig {
        state_path,
        driver_cmd: Some("true".to_string()),
        max_retries: 1,
        timeout_secs: 5,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: true,
        guard_commands: true,
        ..Default::default()
    })
    .expect("runner");
    let _ = runner.run_step();

    let hooks = std::fs::read_to_string(repo.join(".agents").join("hooks.json"))
        .expect("hooks.json must be provisioned");
    for fragment in ["guard pre-tool-use", "guard pre-command"] {
        let idx = hooks.find(fragment).unwrap_or_else(|| panic!("missing {fragment}: {hooks}"));
        let line_start = hooks[..idx].rfind('"').expect("command string start");
        let command = &hooks[line_start..idx];
        assert!(
            command.contains('/'),
            "hook must invoke an absolute path, not a bare name resolved through PATH: {hooks}"
        );
    }
}

#[test]
fn test_daemon_config_default_read_threshold_lines() {
    // The measured default (FINDINGS 13.21). Two independent real-task traces, a Go repo and
    // a TypeScript one, replayed at 17 thresholds, both minimise at exactly 400. The earlier
    // 800 came from a single-file replay and was worse than the 200 it replaced on katex.
    let config = DaemonConfig::default();
    assert_eq!(
        config.read_threshold_lines,
        superstate_core::guard::DEFAULT_READ_THRESHOLD_LINES,
        "DaemonConfig::default().read_threshold_lines must come from the measured constant"
    );
    assert_eq!(
        superstate_core::guard::DEFAULT_READ_THRESHOLD_LINES,
        400,
        "the measured default is 400; changing it is an experiment, not a refactor"
    );
}

#[test]
fn test_daemon_config_deserialize_defaults_read_threshold_lines() {
    // A config written before this field existed must not silently fall back to 0, which
    // would shunt every read and cost roughly twice what no budget at all costs.
    let json = r#"{"state_path": ".agent-state/state.json", "max_retries": 3, "timeout_secs": 120}"#;
    let config: DaemonConfig = serde_json::from_str(json).expect("valid DaemonConfig JSON");
    assert_eq!(
        config.read_threshold_lines,
        superstate_core::guard::DEFAULT_READ_THRESHOLD_LINES,
        "deserialized DaemonConfig without read_threshold_lines must take the measured default"
    );
}

#[test]
fn test_daemon_hook_carries_the_read_cache_ablation() {
    // The switch exists so a paired run can measure the mechanism. If `read_cache: false`
    // did not reach the provisioned hook, the ablation arm would silently run the treatment
    // -- the exact shape of the six arm-parity defects in this project's history.
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");
    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Ablation".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("test -f driver_evidence.txt".to_string()),
        }),
        no_worktree: true,
        goal: Some("Ablate the read cache".to_string()),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some("echo 'driver ran' > driver_evidence.txt".to_string()),
        max_retries: 1,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        rollback_policy: Default::default(),
        shunt_driver_output: false,
        guard_file_reads: true,
        read_threshold_lines: 640,
        read_cache: false,
        guard_commands: false,
        impact_slicing: false,
    };
    let mut runner = DaemonRunner::new(config).unwrap();
    let _ = runner.step();

    let hooks = fs::read_to_string(dir.path().join(".agents/hooks.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&hooks).unwrap();
    let cmd = json["superstate-file-guard"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        cmd.ends_with("--read-cache false"),
        "an ablated run must provision the hook with the cache OFF, got {cmd:?}"
    );
}

#[test]
fn test_read_cache_defaults_off_after_exp081() {
    // EXP-081 measured +37% and +55% input tokens with the cache ON, because 89-91% of
    // suppressions were immediately insisted on: a worker that has decided it needs lines
    // 120-180 needs them, and a PreToolUse hook cannot prevent the call it refuses -- the
    // per-call preamble is already spent by the time the hook runs.
    //
    // Guards the default, not the mechanism. Turning this back on is an experiment and needs
    // evidence that the insist rate can be moved, which nothing so far provides.
    assert!(
        !DaemonConfig::default().read_cache,
        "the read cache is a measured regression and must stay off by default (FINDINGS 13.25)"
    );

    let json = r#"{"state_path": ".agent-state/state.json", "max_retries": 3, "timeout_secs": 120}"#;
    let config: DaemonConfig = serde_json::from_str(json).expect("valid DaemonConfig JSON");
    assert!(
        !config.read_cache,
        "a config written before this field existed must not silently enable the regression"
    );
}
