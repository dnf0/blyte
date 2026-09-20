//! Unit tests for driver preset expansion in superstate-core.
//!
//! Validates autonomous flag expansion for Claude and AGY headless workers.

use superstate_core::daemon::resolve_driver_command;

#[test]
fn test_claude_bare_preset_expansion() {
    // Exact "claude" should expand to include autonomous headless execution flags
    let result = resolve_driver_command("claude");
    assert_eq!(result, "claude -p --dangerously-skip-permissions");
}

#[test]
fn test_claude_with_model_flag_expansion() {
    // "claude" with custom model should append missing autonomous execution flags
    let result = resolve_driver_command("claude --model sonnet-4");
    assert_eq!(
        result,
        "claude --model sonnet-4 -p --dangerously-skip-permissions"
    );
}

#[test]
fn test_claude_existing_flags_preserved() {
    // Commands that already supply required flags must remain unchanged to avoid flag duplication
    let cmd = "claude -p --dangerously-skip-permissions --model haiku";
    let result = resolve_driver_command(cmd);
    assert_eq!(result, cmd);
}

#[test]
fn test_claude_alternative_flags_preserved() {
    // Support alternative headless flags (--print and --permission-mode bypassPermissions)
    let cmd = "claude --print --permission-mode bypassPermissions --model opus";
    let result = resolve_driver_command(cmd);
    assert_eq!(result, cmd);
}

#[test]
fn test_agy_bare_preset_expansion() {
    // Exact "agy" should expand to include autonomous headless execution flags
    let result = resolve_driver_command("agy");
    assert_eq!(result, "agy -p --dangerously-skip-permissions");
}

#[test]
fn test_agy_with_model_flag_expansion() {
    // "agy" with custom model should append missing autonomous execution flags
    let result = resolve_driver_command("agy --model gemini-3.8-pro");
    assert_eq!(
        result,
        "agy --model gemini-3.8-pro -p --dangerously-skip-permissions"
    );
}

#[test]
fn test_agy_existing_flags_preserved() {
    // Commands that already supply required flags must remain unchanged
    let cmd = "agy -p --dangerously-skip-permissions --model flash";
    let result = resolve_driver_command(cmd);
    assert_eq!(result, cmd);
}

#[test]
fn test_custom_command_unchanged() {
    // Non-preset commands must be passed through untouched
    let cmd = "python3 ./worker.py --batch 10";
    let result = resolve_driver_command(cmd);
    assert_eq!(result, cmd);
}

#[test]
fn test_custom_command_with_claude_or_agy_prefix_unchanged() {
    // Ensure custom binaries that share a prefix with presets (e.g. claude-code, agyd)
    // are not mistakenly expanded as standard Claude or AGY presets.
    let custom_cmds = [
        "claude-code --model sonnet",
        "claudette --some-flag",
        "agy-daemon --port 8080",
        "agyd start",
    ];
    for cmd in custom_cmds {
        let result = resolve_driver_command(cmd);
        assert_eq!(result, cmd);
    }
}

#[test]
fn test_claude_partial_flags_expansion() {
    // When partial autonomous flags are present, only the missing flags should be appended
    assert_eq!(
        resolve_driver_command("claude -p"),
        "claude -p --dangerously-skip-permissions"
    );
    assert_eq!(
        resolve_driver_command("claude --print"),
        "claude --print --dangerously-skip-permissions"
    );
    assert_eq!(
        resolve_driver_command("claude --dangerously-skip-permissions"),
        "claude --dangerously-skip-permissions -p"
    );
    assert_eq!(
        resolve_driver_command("claude --permission-mode bypassPermissions"),
        "claude --permission-mode bypassPermissions -p"
    );
    assert_eq!(
        resolve_driver_command("claude --permission-mode=bypassPermissions"),
        "claude --permission-mode=bypassPermissions -p"
    );
}

#[test]
fn test_agy_does_not_accept_claude_permission_mode() {
    // Claude's --permission-mode bypassPermissions must not satisfy AGY's permission requirement
    let cmd = "agy -p --permission-mode bypassPermissions";
    let result = resolve_driver_command(cmd);
    assert_eq!(
        result,
        "agy -p --permission-mode bypassPermissions --dangerously-skip-permissions"
    );

    let cmd_bare = "agy --permission-mode bypassPermissions";
    let result_bare = resolve_driver_command(cmd_bare);
    assert_eq!(
        result_bare,
        "agy --permission-mode bypassPermissions -p --dangerously-skip-permissions"
    );
}

#[test]
fn test_empty_or_whitespace_driver() {
    // Empty strings, whitespace-only commands, and trailing whitespace should be handled safely
    assert_eq!(resolve_driver_command(""), "");
    assert_eq!(resolve_driver_command("   "), "   ");
    assert_eq!(resolve_driver_command("\t\n "), "\t\n ");

    // Preset with trailing whitespace must trim before appending to avoid double spaces
    assert_eq!(
        resolve_driver_command("claude "),
        "claude -p --dangerously-skip-permissions"
    );
    assert_eq!(
        resolve_driver_command("claude -p "),
        "claude -p --dangerously-skip-permissions"
    );
}

#[test]
fn test_daemon_runner_exports_superstate_worktree_path() {
    // Verify SUPERSTATE_WORKTREE_PATH is exported to child driver processes
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join(".agent-state/state.json");

    let milestones = vec![superstate_core::models::MilestoneSpec {
        title: "Worktree Env Test".to_string(),
        verify_cmd: "test -f env_ok.txt".to_string(),
        files: vec![],
        ..Default::default()
    }];

    let state = superstate_core::state_machine::init_state_milestones(
        Some("Env Test"),
        milestones,
        None,
        true,
    )
    .unwrap();
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    let config = superstate_core::daemon::DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(
            "if [ -n \"$SUPERSTATE_WORKTREE_PATH\" ]; then touch env_ok.txt; fi".to_string(),
        ),
        max_retries: 3,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: false,
        shunt_driver_output: false,
        impact_slicing: false,
    };

    let mut runner = superstate_core::daemon::DaemonRunner::new(config).unwrap();
    let outcome = runner.run_step().unwrap();

    assert!(matches!(
        outcome,
        superstate_core::daemon::DaemonStepOutcome::Advanced { .. }
    ));
}
