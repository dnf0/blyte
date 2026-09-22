use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_cli_inspect_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", "--help"])
        .output()
        .expect("failed to run superstate inspect --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--symbol"));
    assert!(stdout.contains("--outline"));
    assert!(stdout.contains("--lines"));
    assert!(stdout.contains("--json"));
}

#[test]
fn test_cli_inspect_file_not_found() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", "/path/to/nonexistent/file.rs"])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: file not found at /path/to/nonexistent/file.rs"));
}

#[test]
fn test_cli_inspect_unsupported_language_ast() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("document.txt");
    fs::write(&file_path, "Hello world\nThis is plain text\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", file_path.to_str().unwrap()])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: unsupported language for AST parsing in '"));
    assert!(stderr.contains("Use --lines <start:end>"));
}

#[test]
fn test_cli_inspect_lines_valid() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("sample.txt");
    let content = "line 1\nline 2\nline 3\nline 4\nline 5\n";
    fs::write(&file_path, content).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", file_path.to_str().unwrap(), "--lines", "2:4"])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("=== Lines 2-4 in {} ===", file_path.display())));
    assert!(stdout.contains("2: line 2"));
    assert!(stdout.contains("3: line 3"));
    assert!(stdout.contains("4: line 4"));
    assert!(!stdout.contains("1: line 1"));
    assert!(!stdout.contains("5: line 5"));
}

#[test]
fn test_cli_inspect_lines_invalid_syntax() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("sample.txt");
    fs::write(&file_path, "line 1\nline 2\n").unwrap();

    // Not numbers
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", file_path.to_str().unwrap(), "--lines", "foo:bar"])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr
        .contains("Error: invalid line range 'foo:bar'. Expected format 'start:end' (e.g. 10:50)"));

    // 0-indexed (start is 0)
    let output_zero = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", file_path.to_str().unwrap(), "--lines", "0:5"])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output_zero.status.code(), Some(1));
    let stderr_zero = String::from_utf8_lossy(&output_zero.stderr);
    assert!(stderr_zero
        .contains("Error: invalid line range '0:5'. Expected format 'start:end' (e.g. 10:50)"));

    // start > end
    let output_reversed = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", file_path.to_str().unwrap(), "--lines", "10:5"])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output_reversed.status.code(), Some(1));
    let stderr_reversed = String::from_utf8_lossy(&output_reversed.stderr);
    assert!(stderr_reversed
        .contains("Error: invalid line range '10:5'. Expected format 'start:end' (e.g. 10:50)"));
}

#[test]
fn test_cli_inspect_outline_default_text() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("worker.rs");
    let code =
        "pub struct Worker {\n    id: u64,\n}\n\npub fn execute() {\n    println!(\"work\");\n}\n";
    fs::write(&file_path, code).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["inspect", file_path.to_str().unwrap()])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("=== Outline: {} (Rust) ===", file_path.display())));
    assert!(stdout.contains("- [Struct] Worker [lines 1-3]"));
    assert!(stdout.contains("- [Function] execute [lines 5-7]"));
}

#[test]
fn test_cli_inspect_outline_json() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("worker.rs");
    let code =
        "pub struct Worker {\n    id: u64,\n}\n\npub fn execute() {\n    println!(\"work\");\n}\n";
    fs::write(&file_path, code).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "inspect",
            file_path.to_str().unwrap(),
            "--outline",
            "--json",
        ])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON array");
    assert!(val.is_array());
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "Worker");
    assert_eq!(arr[1]["name"], "execute");
}

#[test]
fn test_cli_inspect_symbol_found_text() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("worker.rs");
    let code =
        "pub struct Worker {\n    id: u64,\n}\n\npub fn execute() {\n    println!(\"work\");\n}\n";
    fs::write(&file_path, code).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "inspect",
            file_path.to_str().unwrap(),
            "--symbol",
            "execute",
        ])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!(
        "=== Symbol: execute [lines 5-7] in {} ===",
        file_path.display()
    )));
    assert!(stdout.contains("5: pub fn execute() {"));
    assert!(stdout.contains("6:     println!(\"work\");"));
    assert!(stdout.contains("7: }"));
    assert!(!stdout.contains("1: pub struct Worker"));
}

#[test]
fn test_cli_inspect_symbol_found_json() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("worker.rs");
    let code =
        "pub struct Worker {\n    id: u64,\n}\n\npub fn execute() {\n    println!(\"work\");\n}\n";
    fs::write(&file_path, code).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "inspect",
            file_path.to_str().unwrap(),
            "--symbol",
            "execute",
            "--json",
        ])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON object");
    assert!(val.is_object());
    assert_eq!(val["name"], "execute");
    assert_eq!(val["kind"], "Function");
    assert_eq!(val["start_line"], 5);
    assert_eq!(val["end_line"], 7);
}

#[test]
fn test_cli_inspect_symbol_not_found() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("worker.rs");
    let code =
        "pub struct Worker {\n    id: u64,\n}\n\npub fn execute() {\n    println!(\"work\");\n}\n";
    fs::write(&file_path, code).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "inspect",
            file_path.to_str().unwrap(),
            "--symbol",
            "missing_func",
        ])
        .output()
        .expect("failed to run superstate inspect");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&format!(
        "Error: symbol 'missing_func' not found in {}. Available symbols: [Worker, execute]",
        file_path.display()
    )));
}
