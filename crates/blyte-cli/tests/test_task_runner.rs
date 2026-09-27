use blyte_cli::cli::Cli;
use blyte_cli::task_runner::{run_task, run_task_with_state_and_writer, TaskConfig};
use clap::Parser;
use std::sync::Mutex;

/// Global lock synchronizing tests that mutate environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_task_config_from_cli() {
    let cli = Cli::parse_from([
        "blyte",
        "solve the bug",
        "--gate",
        "cargo test",
        "--context",
        "folded",
        "--model",
        "gemini-3.8-flash",
        "--container",
        "docker-123",
        "--max-turns",
        "42",
        "--max-gates",
        "7",
        "--rollback=false",
        "--sandbox",
        "off",
    ]);

    let config = TaskConfig::from(&cli);
    assert_eq!(config.prompt, "solve the bug");
    assert_eq!(config.gate.as_deref(), Some("cargo test"));
    assert_eq!(config.context_strategy, "folded");
    assert_eq!(config.model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(config.container.as_deref(), Some("docker-123"));
    assert_eq!(config.max_turns, 42);
    assert_eq!(config.max_gates, 7);
    assert!(!config.rollback);
    assert_eq!(config.sandbox, "off");
    assert_eq!(
        config.root,
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    );
}

#[test]
fn test_task_config_defaults() {
    let cli = Cli::parse_from(["blyte"]);
    let config = TaskConfig::from(&cli);
    assert_eq!(config.prompt, "");
    assert_eq!(config.gate, None);
    assert_eq!(config.context_strategy, "folded");
    assert_eq!(config.model, None);
    assert_eq!(config.container, None);
    assert_eq!(config.max_turns, 25);
    assert_eq!(config.max_gates, 5);
    assert!(config.rollback);
    assert_eq!(config.sandbox, "on");
    assert_eq!(
        config.root,
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    );
}

#[test]
fn test_run_task_missing_api_key_returns_clear_error() {
    let _guard = ENV_LOCK.lock().unwrap();

    // Ensure all provider keys are unset for deterministic fallback
    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("OLLAMA_HOST");

    let config = TaskConfig {
        prompt: "echo hello".to_string(),
        gate: None,
        context_strategy: "folded".to_string(),
        model: None,
        container: None,
        max_turns: 10,
        max_gates: 2,
        rollback: true,
        root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
    };

    let result = run_task(config);
    assert!(result.is_err(), "Expected error when API key is missing");
    let err = result.unwrap_err();
    assert!(
        err.contains("GEMINI_API_KEY"),
        "Error message should mention GEMINI_API_KEY: {err}"
    );
}

#[test]
fn test_run_task_missing_api_key_for_specified_provider() {
    let _guard = ENV_LOCK.lock().unwrap();

    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("OLLAMA_HOST");

    let anthropic_config = TaskConfig {
        prompt: "echo hello".to_string(),
        gate: None,
        context_strategy: "folded".to_string(),
        model: Some("claude-3-7-sonnet".to_string()),
        container: None,
        max_turns: 10,
        max_gates: 2,
        rollback: true,
        root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
    };

    let err = run_task(anthropic_config).unwrap_err();
    assert!(
        err.contains("ANTHROPIC_API_KEY"),
        "Error message should mention ANTHROPIC_API_KEY: {err}"
    );

    let ollama_config = TaskConfig {
        prompt: "echo hello".to_string(),
        gate: None,
        context_strategy: "folded".to_string(),
        model: Some("qwen2.5-coder:32b".to_string()),
        container: None,
        max_turns: 10,
        max_gates: 2,
        rollback: true,
        root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
    };

    let err = run_task(ollama_config).unwrap_err();
    assert!(
        err.contains("OLLAMA_API_KEY"),
        "Error message should mention OLLAMA_API_KEY: {err}"
    );
}

#[test]
fn test_run_task_with_state_and_writer_preserves_state() {
    let _guard = ENV_LOCK.lock().unwrap();

    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("OLLAMA_HOST");

    let config = TaskConfig {
        prompt: "task goal".to_string(),
        gate: Some("echo 1".to_string()),
        context_strategy: "folded".to_string(),
        model: None,
        container: None,
        max_turns: 5,
        max_gates: 1,
        rollback: false,
        root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
    };

    let mut state = blyte_agent::state::State::new("custom goal", "echo 1");
    let mut output = Vec::new();

    let result = run_task_with_state_and_writer(&config, &mut state, &mut output);
    assert!(result.is_err());
    assert_eq!(state.orchestrator.goal, "custom goal");
    assert_eq!(state.orchestrator.verification_cmd, "echo 1");
}

#[test]
fn test_repl_stream_prompt_dispatch() {
    let _guard = ENV_LOCK.lock().unwrap();

    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("OLLAMA_HOST");

    let config = blyte_cli::repl::ReplConfig::default();
    let session = blyte_cli::repl::ReplSession::new(&config);

    let input = b"Implement feature X\n/exit\n";
    let reader = std::io::Cursor::new(input);
    let mut writer = Vec::new();

    let res = blyte_cli::repl::run_repl_stream(reader, &mut writer, session);
    assert!(res.is_ok());

    let output = String::from_utf8(writer).unwrap();
    assert!(output.contains("Dispatching turn for prompt: Implement feature X"));
    assert!(output.contains("GEMINI_API_KEY"));
}
