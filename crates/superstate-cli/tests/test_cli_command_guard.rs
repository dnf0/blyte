use std::io::Write;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;
use tempfile::tempdir;

fn run_command_guard(stdin_data: &str, extra_args: &[&str]) -> (i32, String, serde_json::Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_superstate"));
    cmd.args(["guard", "pre-command"]);
    cmd.args(extra_args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn superstate");
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data.as_bytes());
    }
    let output = child.wait_with_output().expect("failed to wait on child");
    let code = output.status.code().unwrap_or(-1);
    let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
    let parsed: serde_json::Value = serde_json::from_str(stdout_str.trim()).unwrap_or_else(|e| {
        panic!(
            "failed to parse stdout as JSON: '{}', error: {}, stderr: '{}'",
            stdout_str,
            e,
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (code, stdout_str, parsed)
}

#[test]
fn test_empty_stdin_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let (code, stdout, json) = run_command_guard(
        "",
        &["--state-path", state_file.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
    assert!(stdout.ends_with('\n'), "stdout must end with newline");
}

#[test]
fn test_invalid_json_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let (code, _, json) = run_command_guard(
        "this is not valid json {{{",
        &["--state-path", state_file.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_non_polling_tool_call_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");

    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": "/some/file.rs"
            }
        }
    })
    .to_string();

    let (code, _, json) = run_command_guard(
        &payload,
        &["--state-path", state_file.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_run_command_first_invocation_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");

    let payload = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "ps aux"
            }
        }
    })
    .to_string();

    let (code, _, json) = run_command_guard(
        &payload,
        &["--state-path", state_file.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_run_command_second_polling_invocation_is_throttled_then_allowed() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "ps aux"
            }
        }
    })
    .to_string();

    // First invocation is allowed
    let (code1, _, json1) = run_command_guard(&payload, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Second immediate invocation must be denied
    let (code2, _, json2) = run_command_guard(&payload, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");
    assert!(
        json2.get("reason").is_none(),
        "a throttled allow carries no refusal text: {json2}"
    );
}

#[test]
fn test_run_command_distinct_command_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload1 = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "cargo build"
            }
        }
    })
    .to_string();

    let payload2 = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "cargo test"
            }
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload1, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Distinct command allowed immediately
    let (code2, _, json2) = run_command_guard(&payload2, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    assert_eq!(json2["decision"], "allow");
}

#[test]
fn test_run_command_exact_duplicate_whitespace_normalization() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload1 = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "cargo   check   --all"
            }
        }
    })
    .to_string();

    let payload2 = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "cargo check --all"
            }
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload1, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Normalized duplicate command denied
    let (code2, _, json2) = run_command_guard(&payload2, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");
}

#[test]
fn test_debounce_sec_customization() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "ps aux"
            }
        }
    })
    .to_string();

    let args = [
        "--state-path",
        state_path,
        "--debounce-sec",
        "1",
    ];

    // First invocation allows
    let (code1, _, json1) = run_command_guard(&payload, &args);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Immediate second invocation denies
    let (code2, _, json2) = run_command_guard(&payload, &args);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");

    // Sleep past the 1-second debounce window
    sleep(Duration::from_millis(1100));

    // Third invocation should now be allowed
    let (code3, _, json3) = run_command_guard(&payload, &args);
    assert_eq!(code3, 0);
    assert_eq!(json3["decision"], "allow");
}

#[test]
fn test_manage_task_status_blocks_within_cooldown() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "manage_task",
            "args": {
                "Action": "status",
                "TaskId": "task-42"
            }
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    let (code2, _, json2) = run_command_guard(&payload, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");
}

#[test]
fn test_manage_task_list_shares_cooldown_with_status() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload_status = serde_json::json!({
        "toolCall": {
            "name": "manage_task",
            "args": {
                "Action": "status",
                "TaskId": "task-42"
            }
        }
    })
    .to_string();

    let payload_list = serde_json::json!({
        "toolCall": {
            "name": "manage_task",
            "args": {
                "Action": "list"
            }
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload_status, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // "list" shares cooldown category with "status"
    let (code2, _, json2) = run_command_guard(&payload_list, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");
}

#[test]
fn test_manage_task_non_polling_action_allows() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload_kill = serde_json::json!({
        "toolCall": {
            "name": "manage_task",
            "args": {
                "Action": "kill",
                "TaskId": "task-99"
            }
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload_kill, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Second kill is still allowed (non-polling action)
    let (code2, _, json2) = run_command_guard(&payload_kill, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    assert_eq!(json2["decision"], "allow");
}

#[test]
fn test_procfs_polling_blocks_within_cooldown() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    let payload1 = serde_json::json!({
        "toolCall": {
            "name": "bash",
            "args": {
                "command": "cat /proc/4211/status"
            }
        }
    })
    .to_string();

    let payload2 = serde_json::json!({
        "toolCall": {
            "name": "bash",
            "args": {
                "command": "cat /proc/8842/cmdline"
            }
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload1, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Second procfs poll within cooldown denies even with different PID
    let (code2, _, json2) = run_command_guard(&payload2, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");
}

#[test]
fn test_flexible_payload_formats() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("guard_history.json");
    let state_path = state_file.to_str().unwrap();

    // Claude Code style: tool_name and tool_input
    let payload_claude = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {
            "command": "ps -ef"
        }
    })
    .to_string();

    let (code1, _, json1) = run_command_guard(&payload_claude, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code1, 0);
    assert_eq!(json1["decision"], "allow");

    // Snake case tool_call format with same inspection command
    let payload_snake = serde_json::json!({
        "tool_call": {
            "name": "run_command",
            "args": {
                "command_line": "top"
            }
        }
    })
    .to_string();

    // "top" is in process inspection group, same as "ps -ef"
    let (code2, _, json2) = run_command_guard(&payload_snake, &["--state-path", state_path, "--debounce-sec", "1"]);
    assert_eq!(code2, 0);
    // Throttled, not denied. The hook sleeps out the remaining cooldown and then allows: the
    // agent's poll rate is capped exactly as before, but it never receives a refusal it can
    // only respond to by ending its turn. Denying here killed runs in EXP-071/074/076.
    assert_eq!(json2["decision"], "allow");
}
