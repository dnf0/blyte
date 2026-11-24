use blyte_cli::cli::Cli;
use blyte_cli::task_runner::{
    run_task, run_task_with_state_and_events, run_task_with_state_and_writer, TaskConfig,
};
use clap::Parser;
use std::io::{Read, Write};
use std::net::TcpListener;
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
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
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
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
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
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
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
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
    };

    let mut state = blyte_agent::state::State::new("custom goal", "echo 1");
    let mut output = Vec::new();

    let result = run_task_with_state_and_writer(&config, &mut state, &mut output);
    assert!(result.is_err());
    assert_eq!(state.brief.goal, "custom goal");
    assert_eq!(state.brief.verification_cmd, "echo 1");
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

struct EnvRestore {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvRestore {
    fn capture(keys: &[&'static str]) -> Self {
        Self {
            saved: keys.iter().map(|&k| (k, std::env::var(k).ok())).collect(),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

fn mock_openai_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let body = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    port
}

#[test]
fn test_run_task_with_state_and_events_emits_turn() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _restore = EnvRestore::capture(&["OPENAI_BASE_URL", "OPENAI_API_KEY", "BLYTE_USAGE_LOG"]);

    let port = mock_openai_server();
    std::env::set_var("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"));
    std::env::set_var("OPENAI_API_KEY", "test-key");
    std::env::set_var("BLYTE_USAGE_LOG", "off");

    let dir = tempfile::tempdir().unwrap();
    let config = TaskConfig {
        prompt: "say done".to_string(),
        gate: None,
        context_strategy: "conversational".to_string(),
        model: Some("openai/gpt-test".to_string()),
        container: None,
        max_turns: 3,
        max_gates: 1,
        rollback: false,
        root: dir.path().to_path_buf(),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "off".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
    };

    let mut state = blyte_agent::state::State::new("say done", "");
    let turns = Mutex::new(Vec::new());
    let result = run_task_with_state_and_events(&config, &mut state, &|event| {
        if let blyte_agent::run::Event::Turn { n, text, .. } = event {
            turns.lock().unwrap().push((*n, (*text).to_string()));
        }
    });

    let (outcome, _report) = result.expect("run_task_with_state_and_events should succeed");
    assert_eq!(outcome, blyte_agent::run::Outcome::Verified);
    let seen_turns = turns.into_inner().unwrap();
    assert!(
        !seen_turns.is_empty(),
        "expected at least one Turn event to be passed to callback"
    );
    assert_eq!(seen_turns[0].0, 1);
    assert_eq!(seen_turns[0].1, "done");
}
