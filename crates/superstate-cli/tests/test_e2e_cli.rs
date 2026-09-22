use std::io::Write;
use std::process::Command;

#[test]
fn test_cli_binary_help_screen() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("--help")
        .output()
        .expect("Failed to execute superstate --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("superstate"));
    assert!(stdout.contains("[PROMPT]"));
    assert!(stdout.contains("--gate"));
    assert!(stdout.contains("--context"));
    assert!(stdout.contains("--model"));
    assert!(stdout.contains("--container"));
    assert!(stdout.contains("--max-turns"));
    assert!(stdout.contains("--max-gates"));
    assert!(stdout.contains("--rollback"));
}

#[test]
fn test_cli_empty_prompt_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("")
        .output()
        .expect("Failed to execute superstate with empty prompt");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: prompt cannot be empty"));
}

#[test]
fn test_cli_headless_missing_api_key() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("fix math bug")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .expect("Failed to execute superstate in headless mode");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"));
}

#[test]
fn test_cli_subcommand_dispatch() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("doctor")
        .output()
        .expect("Failed to execute superstate doctor");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("Verification Handshake")
            || combined.contains("Verification Handshake Passed")
    );
}

#[test]
fn test_cli_interactive_repl_stdin_driving() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn superstate REPL");

    {
        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        stdin
            .write_all(b"/help\n/gate cargo test\n/state\n/exit\n")
            .expect("Failed to write commands to stdin");
    }

    let output = child.wait_with_output().expect("Failed to read output");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Superstate Interactive Harness"));
    assert!(stdout.contains("Available Slash Commands:"));
    assert!(stdout.contains("Gate set to: cargo test"));
    assert!(stdout.contains("=== Superstate SKILL.state ==="));
    assert!(stdout.contains("Orchestrator:"));
    assert!(stdout.contains("Runtime:"));
    assert!(stdout.contains("Agent:"));
}
