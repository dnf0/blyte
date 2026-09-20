use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::tempdir;

#[test]
fn test_cli_shunt_small_output_exit_code_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "echo", "hello world"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello world"));
    assert!(!stdout.contains("[superstate-shunt]"));
}

#[test]
fn test_shunt_under_threshold_outputs_verbatim() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "echo \"hello verbatim\""])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello verbatim"));
    assert!(!stdout.contains("[superstate-shunt]"));
}

#[test]
fn test_cli_shunt_propagates_non_zero_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "exit 42"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(42));
}

#[test]
fn test_shunt_preserves_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "exit 42"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(42));
}

#[test]
fn test_shunt_preserves_exit_code_127() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "exit 127"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(127));
}

#[test]
fn test_cli_shunt_large_output_truncates_and_saves_log() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "shunt",
            "--threshold-lines",
            "10",
            "--",
            "sh",
            "-c",
            "for i in $(seq 1 30); do echo line $i; done",
        ])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[superstate-shunt] Command succeeded (exit 0)"));
    assert!(stdout.contains("Full log:"));
}

#[test]
fn test_shunt_over_threshold_distills_success() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "shunt",
            "--threshold-lines",
            "5",
            "--",
            "sh",
            "-c",
            "for i in $(seq 1 20); do echo \"line $i\"; done",
        ])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[superstate-shunt] Command succeeded (exit 0)"));
    assert!(stdout.contains("Full log:"));
}

#[test]
fn test_shunt_failure_distills_with_excerpt() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "shunt",
            "--threshold-lines",
            "5",
            "--",
            "sh",
            "-c",
            "for i in $(seq 1 20); do echo \"line $i\"; done; echo \"FAIL: assertion failed\"; exit 1",
        ])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[superstate-shunt] Command failed with exit code 1"));
    assert!(stdout.contains("FAIL: assertion failed"));
    assert!(stdout.contains("Full log:"));
}

#[test]
fn test_shunt_forwards_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "cat; exit 17"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn superstate shunt");

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(b"test stdin content\n")
            .expect("failed to write to stdin");
    }

    let output = child
        .wait_with_output()
        .expect("failed to wait on child process");

    assert_eq!(output.status.code(), Some(17));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test stdin content"));
}

#[test]
fn test_shunt_large_stdin_streaming() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "wc -c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn superstate shunt");

    let chunk = vec![b'A'; 65536];
    if let Some(mut stdin) = child.stdin.take() {
        // Write 128 KB (exceeding standard 64 KB OS pipe buffer)
        stdin.write_all(&chunk).expect("failed to write chunk 1");
        stdin.write_all(&chunk).expect("failed to write chunk 2");
    }

    let output = child
        .wait_with_output()
        .expect("failed to wait on child process");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let bytes_reported: usize = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("Failed to parse byte count from output: '{}'", stdout));
    assert_eq!(bytes_reported, 131072);
}

#[test]
fn test_shunt_custom_logs_dir() {
    let dir = tempdir().unwrap();
    let custom_logs = dir.path().join("my-custom-logs");

    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "shunt",
            "--threshold-lines",
            "5",
            "--logs-dir",
            custom_logs.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "for i in $(seq 1 20); do echo \"line $i\"; done",
        ])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[superstate-shunt] Command succeeded (exit 0)"));
    assert!(stdout.contains(custom_logs.to_str().unwrap()));
    assert!(custom_logs.exists());
    let log_files: Vec<_> = std::fs::read_dir(&custom_logs)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert_eq!(log_files.len(), 1);
}

#[cfg(unix)]
#[test]
fn test_cli_shunt_signal_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "kill -9 $$"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(137));
}

#[test]
fn test_shunt_bidirectional_streaming() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "tr", "a-z", "A-Z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn superstate shunt");

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(b"hello world streaming\n")
            .expect("failed to write to stdin");
    }

    let output = child
        .wait_with_output()
        .expect("failed to wait on child process");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("HELLO WORLD STREAMING"));
}

#[test]
fn test_shunt_nonexistent_command_fails_cleanly() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "nonexistent_binary_cmd_xyz_987"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to spawn command"));
}
