use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::tempdir;

fn run_guard(stdin_data: &str, extra_args: &[&str]) -> (i32, String, serde_json::Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_superstate"));
    cmd.args(["guard", "pre-tool-use"]);
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
    let (code, stdout, json) = run_guard("", &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
    assert!(stdout.ends_with('\n'), "stdout must end with newline");
}

#[test]
fn test_invalid_json_allows() {
    let (code, _, json) = run_guard("this is not valid json {{{", &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_non_file_tool_call_allows() {
    let payload = serde_json::json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "command": "cargo test"
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_missing_or_empty_path_allows() {
    let payload_missing = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {}
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload_missing, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");

    let payload_empty = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": "   "
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload_empty, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_non_existent_file_allows() {
    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": "/non/existent/file.rs"
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_small_file_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("small.py");
    let content = "def hello():\n    return 'world'\n";
    fs::write(&file_path, content).unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": file_path.to_str().unwrap()
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_large_file_unbounded_denies_with_ast_outline() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");

    let mut lines = Vec::new();
    lines.push("class AlphaService:".to_string());
    lines.push("    def process(self):".to_string());
    lines.push("        pass".to_string());
    for i in 4..=250 {
        lines.push(format!("    # filler line {}", i));
    }
    lines.push("def helper():".to_string());
    lines.push("    return 1".to_string());
    fs::write(&file_path, lines.join("\n")).unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": file_path.to_str().unwrap()
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "deny");
    let reason = json["reason"].as_str().unwrap();
    assert!(
        reason.contains("[superstate-guard] Full file read blocked"),
        "reason should mention blocked full read: {}",
        reason
    );
    assert!(
        reason.contains("252 lines exceeds 200-line budget"),
        "reason should mention lines: {}",
        reason
    );
    assert!(
        reason.contains("AST Symbol Outline:"),
        "reason should mention AST outline: {}",
        reason
    );
    assert!(
        reason.contains("AlphaService"),
        "reason should contain class name: {}",
        reason
    );
    assert!(
        reason.contains("helper"),
        "reason should contain function name: {}",
        reason
    );
}

#[test]
fn test_bounded_slice_under_threshold_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");
    let lines: Vec<String> = (1..=250).map(|i| format!("# line {}", i)).collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": file_path.to_str().unwrap(),
                "StartLine": 1,
                "EndLine": 50
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_head_slice_with_only_end_line_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");
    let lines: Vec<String> = (1..=250).map(|i| format!("# line {}", i)).collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    // Only EndLine specified: should normalize start_line to 1
    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": file_path.to_str().unwrap(),
                "EndLine": 50
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}

#[test]
fn test_bounded_slice_exceeding_threshold_denies() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");
    let lines: Vec<String> = (1..=250).map(|i| format!("# line {}", i)).collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": file_path.to_str().unwrap(),
                "StartLine": 1,
                "EndLine": 240
            }
        }
    })
    .to_string();

    let (code, _, json) = run_guard(&payload, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "deny");
}

#[test]
fn test_custom_threshold_lines_parameter() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("medium.txt");
    let lines: Vec<String> = (1..=80).map(|i| format!("line {}", i)).collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    let payload = serde_json::json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": file_path.to_str().unwrap()
            }
        }
    })
    .to_string();

    // Default threshold (200) allows 80 lines
    let (code_default, _, json_default) = run_guard(&payload, &[]);
    assert_eq!(code_default, 0);
    assert_eq!(json_default["decision"], "allow");

    // Custom threshold (50) denies 80 lines
    let (code_custom, _, json_custom) = run_guard(&payload, &["--threshold-lines", "50"]);
    assert_eq!(code_custom, 0);
    assert_eq!(json_custom["decision"], "deny");
    let reason = json_custom["reason"].as_str().unwrap();
    assert!(reason.contains("80 lines exceeds 50-line budget"));
}

#[test]
fn test_flexible_payload_formats_and_tool_names() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");
    let lines: Vec<String> = (1..=250).map(|i| format!("# line {}", i)).collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    // Claude Code style: "View" with "filePath"
    let payload_claude = serde_json::json!({
        "toolCall": {
            "name": "View",
            "args": {
                "filePath": file_path.to_str().unwrap()
            }
        }
    })
    .to_string();
    let (code, _, json) = run_guard(&payload_claude, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "deny");

    // Snake case tool_call: "read_file" with "file_path", "start_line", "end_line"
    let payload_snake = serde_json::json!({
        "tool_call": {
            "name": "read_file",
            "args": {
                "file_path": file_path.to_str().unwrap(),
                "start_line": 10,
                "end_line": 30
            }
        }
    })
    .to_string();
    let (code, _, json) = run_guard(&payload_snake, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");

    // Top-level toolName and args with "file" and "readfile"
    let payload_top_level = serde_json::json!({
        "toolName": "readfile",
        "args": {
            "file": file_path.to_str().unwrap(),
            "offset": 5,
            "limit": 25
        }
    })
    .to_string();
    let (code, _, json) = run_guard(&payload_top_level, &[]);
    assert_eq!(code, 0);
    assert_eq!(json["decision"], "allow");
}
