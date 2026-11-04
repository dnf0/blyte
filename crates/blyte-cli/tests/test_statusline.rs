//! Tests for `blyte statusline`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use blyte_cli::statusline::{base_line, resolve_compact_window, settings_files, CompactWindow};

#[test]
fn test_base_line_full() {
    // 1. base_line for full input (used_percentage: 45, context_window_size: 200000, cost 1.5, model Opus)
    // with compact_window: Window(200000) gives Opus · context 90k of 200k · $1.50 this session.
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
    let result = base_line(&d, CompactWindow::Window(200000));
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
    let result_size = base_line(&d_size, CompactWindow::Window(200000));
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
    let r_65 = base_line(&d_65, CompactWindow::Window(200000));
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
    let r_p_only = base_line(&d_p_only, CompactWindow::Tuned);
    assert_eq!(r_p_only, "claude · context 45%");

    // {} gives claude
    let d_empty = serde_json::json!({});
    assert_eq!(base_line(&d_empty, CompactWindow::Tuned), "claude");
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
        .env("HOME", tmp.path())
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
    let tmp = tempfile::tempdir().unwrap();
    let stdin_payload = r#"{"model":{"display_name":"Opus"}}"#;

    // --after "echo mine"
    {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
        cmd.args(["statusline", "--after", "echo mine"])
            .env("HOME", tmp.path())
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
            .env("HOME", tmp.path())
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
            .env("HOME", tmp.path())
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
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.arg("statusline")
        .env("HOME", tmp.path())
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
    let tmp = tempfile::tempdir().unwrap();
    let pad = "x".repeat(256 * 1024);
    let stdin_payload = format!(r#"{{"model":{{"display_name":"Opus"}},"pad":"{pad}"}}"#);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.args(["statusline", "--after", "exec 0<&-; echo mine"])
        .env("HOME", tmp.path())
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

#[test]
fn test_no_window_anywhere_uses_model_window() {
    // no window anywhere, size 1000000, 30% used: `context 300k of 1000k`, no suggestion; 65%: the suggestion
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nonexistent.json");
    let cw = resolve_compact_window(Some("claude-opus-5-5[1m]"), None, &[&missing]);
    assert_eq!(cw, CompactWindow::Tuned);

    let d_30 = serde_json::json!({
        "model": { "display_name": "Opus", "id": "claude-opus-5-5[1m]" },
        "context_window": {
            "used_percentage": 30,
            "context_window_size": 1000000
        }
    });
    assert_eq!(base_line(&d_30, cw.clone()), "Opus · context 300k of 1000k");

    let d_65 = serde_json::json!({
        "model": { "display_name": "Opus", "id": "claude-opus-5-5[1m]" },
        "context_window": {
            "used_percentage": 65,
            "context_window_size": 1000000
        }
    });
    assert_eq!(
        base_line(&d_65, cw),
        "Opus · context 650k of 1000k — consider /compact or a new session"
    );
}

#[test]
fn test_top_level_user_window() {
    // top-level `autoCompactWindow: 200000` in the user file, size 1000000, 30%: `of 200k` with the suggestion
    let tmp = tempfile::tempdir().unwrap();
    let user_file = tmp.path().join("user-settings.json");
    std::fs::write(&user_file, r#"{"autoCompactWindow": 200000}"#).unwrap();

    let cw = resolve_compact_window(Some("claude-opus-5-5[1m]"), None, &[&user_file]);
    assert_eq!(cw, CompactWindow::Window(200_000));

    let d = serde_json::json!({
        "model": { "display_name": "Opus", "id": "claude-opus-5-5[1m]" },
        "context_window": {
            "used_percentage": 30,
            "context_window_size": 1000000
        }
    });
    assert_eq!(
        base_line(&d, cw),
        "Opus · context 300k of 200k — consider /compact or a new session"
    );
}

#[test]
fn test_model_settings_beats_top_level_in_same_file() {
    // `modelSettings.claude-opus-5-5.autoCompactWindow: 500000` with `model.id`
    // `claude-opus-5-5[1m]` beats a top-level 200000 in the same file;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("settings.json");
    std::fs::write(
        &file,
        r#"{
            "autoCompactWindow": 200000,
            "modelSettings": {
                "claude-opus-5-5": {
                    "autoCompactWindow": 500000
                }
            }
        }"#,
    )
    .unwrap();

    let cw = resolve_compact_window(Some("claude-opus-5-5[1m]"), None, &[&file]);
    assert_eq!(cw, CompactWindow::Window(500_000));

    let d = serde_json::json!({
        "model": { "display_name": "Opus", "id": "claude-opus-5-5[1m]" },
        "context_window": {
            "used_percentage": 25,
            "context_window_size": 1000000
        }
    });
    assert_eq!(base_line(&d, cw), "Opus · context 250k of 500k");

    // Also verify date-suffixed key and model.id matching
    let cw_dated = resolve_compact_window(Some("claude-opus-5-5-20260501[1m]"), None, &[&file]);
    assert_eq!(cw_dated, CompactWindow::Window(500_000));
}

#[test]
fn test_project_file_beats_user_file() {
    // a project-file window beats a user-file window;
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("proj");
    let home_dir = tmp.path().join("home");
    std::fs::create_dir_all(project_dir.join(".claude")).unwrap();
    std::fs::create_dir_all(home_dir.join(".claude")).unwrap();

    let proj_file = project_dir.join(".claude").join("settings.json");
    let user_file = home_dir.join(".claude").join("settings.json");
    std::fs::write(&proj_file, r#"{"autoCompactWindow": 300000}"#).unwrap();
    std::fs::write(&user_file, r#"{"autoCompactWindow": 200000}"#).unwrap();

    let d = serde_json::json!({
        "model": { "display_name": "Opus" },
        "workspace": { "project_dir": project_dir.to_str().unwrap() },
        "context_window": {
            "used_percentage": 20,
            "context_window_size": 1000000
        }
    });

    // Build scope order excluding system /Library path so the test stays inside temp dirs
    let scoped: Vec<PathBuf> = settings_files(&d, Some(&home_dir))
        .into_iter()
        .skip(1)
        .collect();
    let cw = resolve_compact_window(None, None, &scoped);
    assert_eq!(cw, CompactWindow::Window(300_000));
    assert_eq!(
        base_line(&d, cw),
        "Opus · context 200k of 300k — consider /compact or a new session"
    );
}

#[test]
fn test_auto_in_higher_file_beats_number_in_lower_file() {
    // `"auto"` in a higher file beats a number in a lower one, giving the model window;
    let tmp = tempfile::tempdir().unwrap();
    let higher = tmp.path().join("local-settings.json");
    let lower = tmp.path().join("user-settings.json");
    std::fs::write(&higher, r#"{"autoCompactWindow": "auto"}"#).unwrap();
    std::fs::write(&lower, r#"{"autoCompactWindow": 200000}"#).unwrap();

    let cw = resolve_compact_window(Some("claude-opus-5-5[1m]"), None, &[&higher, &lower]);
    assert_eq!(cw, CompactWindow::Tuned);

    let d = serde_json::json!({
        "model": { "display_name": "Opus", "id": "claude-opus-5-5[1m]" },
        "context_window": {
            "used_percentage": 30,
            "context_window_size": 1000000
        }
    });
    assert_eq!(base_line(&d, cw), "Opus · context 300k of 1000k");
}

#[test]
fn test_env_var_beats_files_and_validates() {
    // the environment variable `400k` beats all files; `50k` and `abc` give the `Invalid` text naming
    // the variable;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("settings.json");
    std::fs::write(&file, r#"{"autoCompactWindow": 200000}"#).unwrap();

    let d = serde_json::json!({
        "model": { "display_name": "Opus" },
        "context_window": {
            "used_percentage": 30,
            "context_window_size": 1000000
        }
    });

    // 400k beats the 200000 in file
    let cw_400k = resolve_compact_window(None, Some("400k"), &[&file]);
    assert_eq!(cw_400k, CompactWindow::Window(400_000));
    assert_eq!(
        base_line(&d, cw_400k),
        "Opus · context 300k of 400k — consider /compact or a new session"
    );

    // 50k is out of range -> Invalid naming CLAUDE_CODE_AUTO_COMPACT_WINDOW, no suggestion
    let cw_50k = resolve_compact_window(None, Some("50k"), &[&file]);
    assert!(matches!(cw_50k, CompactWindow::Invalid(_)));
    let line_50k = base_line(&d, cw_50k);
    assert!(
        line_50k.starts_with(
            "Opus · context 300k of 1000k (auto-compact window unknown: CLAUDE_CODE_AUTO_COMPACT_WINDOW"
        ),
        "unexpected line for 50k: {line_50k}"
    );
    assert!(!line_50k.contains("consider /compact"));

    // abc is invalid -> Invalid naming CLAUDE_CODE_AUTO_COMPACT_WINDOW, no suggestion
    let cw_abc = resolve_compact_window(None, Some("abc"), &[&file]);
    assert!(matches!(cw_abc, CompactWindow::Invalid(_)));
    let line_abc = base_line(&d, cw_abc);
    assert!(
        line_abc.starts_with(
            "Opus · context 300k of 1000k (auto-compact window unknown: CLAUDE_CODE_AUTO_COMPACT_WINDOW"
        ),
        "unexpected line for abc: {line_abc}"
    );
    assert!(!line_abc.contains("consider /compact"));
}

#[test]
fn test_broken_json_settings_file_gives_invalid() {
    // a settings file with broken JSON gives the `Invalid` text naming the file.
    let tmp = tempfile::tempdir().unwrap();
    let broken = tmp.path().join("broken-settings.json");
    std::fs::write(&broken, "{ not valid json").unwrap();

    let cw = resolve_compact_window(None, None, &[&broken]);
    assert!(matches!(cw, CompactWindow::Invalid(_)));

    let d = serde_json::json!({
        "model": { "display_name": "Opus" },
        "context_window": {
            "used_percentage": 80,
            "context_window_size": 1000000
        }
    });
    let line = base_line(&d, cw);
    let broken_str = broken.display().to_string();
    assert!(
        line.contains("context 800k of 1000k (auto-compact window unknown: "),
        "unexpected line: {line}"
    );
    assert!(
        line.contains(&broken_str),
        "expected broken file path '{broken_str}' in line: {line}"
    );
    assert!(!line.contains("consider /compact"));
}
