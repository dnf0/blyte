//! Tests for `blyte statusline`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn test_base_line_full() {
    // 1. base_line for full input (used_percentage: 45, context_window_size: 200000, cost 1.5, model Opus)
    // with compact_window: Some(200000) gives Opus · context 90k of 200k · $1.50 this session.
    let d = serde_json::json!({
        "model": {
            "display_name": "Opus"
        },
        "context_window": {
            "used_percentage": 45,
            "context_window_size": 200000
        },
        "cost": {
            "total_cost_usd": 1.5
        }
    });
    let result = blyte_cli::statusline::base_line(&d, Some(200000));
    assert_eq!(result, "Opus · context 90k of 200k · $1.50 this session");

    // Also verify with .size instead of .context_window_size
    let d_size = serde_json::json!({
        "model": {
            "display_name": "Opus"
        },
        "context_window": {
            "used_percentage": 45,
            "size": 200000
        },
        "cost": {
            "total_cost_usd": 1.5
        }
    });
    let result_size = blyte_cli::statusline::base_line(&d_size, Some(200000));
    assert_eq!(
        result_size,
        "Opus · context 90k of 200k · $1.50 this session"
    );
}

#[test]
fn test_base_line_variants() {
    // 2. 65% of 200k gives the consider /compact suffix; percentage without size gives context 45%; {} gives claude.

    // 65% of 200k gives consider /compact suffix
    let d_65 = serde_json::json!({
        "context_window": {
            "used_percentage": 65,
            "context_window_size": 200000
        }
    });
    let r_65 = blyte_cli::statusline::base_line(&d_65, Some(200000));
    assert_eq!(
        r_65,
        "claude · context 130k of 200k — consider /compact or a new session"
    );

    // Percentage without size gives context 45%
    let d_p_only = serde_json::json!({
        "context_window": {
            "used_percentage": 45
        }
    });
    let r_p_only = blyte_cli::statusline::base_line(&d_p_only, None);
    assert_eq!(r_p_only, "claude · context 45%");

    // {} gives claude
    let d_empty = serde_json::json!({});
    assert_eq!(blyte_cli::statusline::base_line(&d_empty, None), "claude");
}

#[test]
fn test_blyte_part() {
    // 3. blyte_part with 0 runs is None; with 2 runs and $0.07 and claude cost 2.5 it is
    // blyte $0.07 (2 runs) · total $2.57; with claude cost None there is no total.
    let t_zero = blyte_cli::cost::Totals {
        orchestrator_usd: 0.0,
        implementers_usd: 0.0,
        runs: 0,
    };
    assert_eq!(blyte_cli::statusline::blyte_part(&t_zero, Some(2.5)), None);

    let t_two = blyte_cli::cost::Totals {
        orchestrator_usd: 0.1,
        implementers_usd: 0.07,
        runs: 2,
    };
    assert_eq!(
        blyte_cli::statusline::blyte_part(&t_two, Some(2.5)),
        Some("blyte $0.07 (2 runs) · total $2.57".to_string())
    );

    assert_eq!(
        blyte_cli::statusline::blyte_part(&t_two, None),
        Some("blyte $0.07 (2 runs)".to_string())
    );
}

#[test]
fn test_binary_with_usage_log() {
    // 4. Through the binary with BLYTE_USAGE_LOG pointing at a temp file holding two entries for session s1
    // (one with dispatch: Some), stdin {"session_id":"s1","model":{"display_name":"Opus"},"cost":{"total_cost_usd":1}}:
    // the output contains Opus and blyte $ and (1 runs), and the exit is 0.
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("usage.jsonl");

    let entry1 = blyte_cli::usage_log::Entry {
        ts: 1000,
        repo: "/repo/a".into(),
        session: Some("s1".into()),
        dispatch: Some("d1".into()),
        research: false,
        model: "m1".into(),
        outcome: "verified".into(),
        turns: 1,
        input: 100,
        cached: 0,
        output: 50,
        thinking: 0,
        cost_usd: Some(0.25),
    };
    let entry2 = blyte_cli::usage_log::Entry {
        ts: 2000,
        repo: "/repo/a".into(),
        session: Some("s1".into()),
        dispatch: None,
        research: false,
        model: "m2".into(),
        outcome: "verified".into(),
        turns: 2,
        input: 200,
        cached: 0,
        output: 100,
        thinking: 0,
        cost_usd: Some(0.10),
    };

    blyte_cli::usage_log::append(&log_path, &entry1).unwrap();
    blyte_cli::usage_log::append(&log_path, &entry2).unwrap();

    let stdin_payload =
        r#"{"session_id":"s1","model":{"display_name":"Opus"},"cost":{"total_cost_usd":1}}"#;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.arg("statusline")
        .env("BLYTE_USAGE_LOG", log_path.as_os_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(stdin_payload.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Opus"), "expected Opus in output: {stdout}");
    assert!(
        stdout.contains("blyte $"),
        "expected 'blyte $' in output: {stdout}"
    );
    assert!(
        stdout.contains("(1 runs)"),
        "expected '(1 runs)' in output: {stdout}"
    );
}

#[test]
fn test_after_flag() {
    // 5. --after "echo mine" prints a line starting with mine; --after "exit 1" falls back to the base
    // line; --after "sleep 5" falls back within 4 seconds.
    let stdin_payload = r#"{"model":{"display_name":"Opus"}}"#;

    // --after "echo mine"
    {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
        cmd.args(["statusline", "--after", "echo mine"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(stdin_payload.as_bytes()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.starts_with("mine"),
            "expected line starting with mine: {stdout}"
        );
    }

    // --after "exit 1" falls back to the base line ("Opus")
    {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
        cmd.args(["statusline", "--after", "exit 1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(stdin_payload.as_bytes()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.trim(), "Opus");
    }

    // --after "sleep 5" falls back within 4 seconds
    {
        let start = Instant::now();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
        cmd.args(["statusline", "--after", "sleep 5"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(stdin_payload.as_bytes()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let elapsed = start.elapsed();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.trim(), "Opus");
        assert!(
            elapsed < Duration::from_secs(4),
            "expected fallback within 4s, took: {:?}",
            elapsed
        );
    }
}

#[test]
fn test_malformed_stdin() {
    // 6. Malformed stdin (not json) prints claude and exits 0.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.arg("statusline")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"not json").unwrap();
    }
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), "claude");
}

#[test]
fn test_after_command_that_does_not_read_stdin() {
    // An --after command that closes its input must not kill blyte with SIGPIPE: the input here is
    // larger than a pipe buffer, so the write always hits the closed pipe.
    let pad = "x".repeat(256 * 1024);
    let stdin_payload = format!(r#"{{"model":{{"display_name":"Opus"}},"pad":"{pad}"}}"#);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.args(["statusline", "--after", "exec 0<&-; echo mine"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(stdin_payload.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("mine"),
        "expected line starting with mine: {stdout}"
    );
}
