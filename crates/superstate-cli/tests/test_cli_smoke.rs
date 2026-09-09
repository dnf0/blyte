use std::process::Command;

#[test]
fn test_cli_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("--version")
        .output()
        .expect("Failed to run superstate --version");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("superstate"));
}

#[test]
fn test_cli_doctor() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("doctor")
        .output()
        .expect("Failed to run superstate doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Verification Handshake Passed"));
}
