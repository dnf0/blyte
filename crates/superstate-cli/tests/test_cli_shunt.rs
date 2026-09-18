use std::process::Command;

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
fn test_cli_shunt_propagates_non_zero_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "exit 42"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(42));
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
