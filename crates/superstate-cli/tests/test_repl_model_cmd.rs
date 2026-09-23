use superstate_cli::repl::{handle_repl_command, ReplConfig, ReplControl, ReplSession};
use superstate_provider::ModelInfo;

fn create_test_session() -> ReplSession {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
    };
    ReplSession::new(&config)
}

#[test]
fn test_slash_help_includes_model() {
    let mut session = create_test_session();
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/help", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("/model"));
    assert!(text.contains("Discover and switch active AI model"));
}

#[test]
fn test_repl_session_default_model_is_gemini_3_8_flash() {
    let session = create_test_session();
    assert_eq!(session.model, "gemini-3.8-flash");
    assert!(session.cached_models.is_empty());
}

#[test]
fn test_slash_model_empty_args_triggers_discovery() {
    let mut session = create_test_session();
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/model", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();

    assert!(text.contains("Discovering available cloud models..."));
    assert!(text.contains("Active model:"));
    assert!(text.contains("To switch: /model <number> or /model <name>"));
}

#[test]
fn test_slash_model_switch_by_numeric_index() {
    let mut session = create_test_session();
    session.cached_models = vec![
        ModelInfo {
            id: "claude-3-7-sonnet-latest".to_string(),
            display_name: "Claude 3.7 Sonnet (Latest)".to_string(),
            description: Some("Frontier hybrid reasoning & coding".to_string()),
            provider: "anthropic".to_string(),
        },
        ModelInfo {
            id: "gemini-3.8-flash".to_string(),
            display_name: "Gemini 3.8 Flash".to_string(),
            description: Some("Next-generation multimodal fast model".to_string()),
            provider: "gemini".to_string(),
        },
        ModelInfo {
            id: "qwen2.5-coder:32b".to_string(),
            display_name: "Qwen 2.5 Coder 32B".to_string(),
            description: Some("Cloud-hosted coding model".to_string()),
            provider: "ollama".to_string(),
        },
    ];

    // Switch to index 1
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/model 1", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.model, "claude-3-7-sonnet-latest");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Switched active model to: claude-3-7-sonnet-latest (anthropic)"));

    // Switch to index 3
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/model 3", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.model, "qwen2.5-coder:32b");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Switched active model to: qwen2.5-coder:32b (ollama)"));
}

#[test]
fn test_slash_model_switch_by_name() {
    let mut session = create_test_session();

    // Switch to Anthropic Claude model by name
    let mut out = Vec::new();
    let res =
        handle_repl_command(&mut session, "/model claude-3-5-sonnet-latest", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.model, "claude-3-5-sonnet-latest");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Switched active model to: claude-3-5-sonnet-latest (Anthropic)"));

    // Switch to Ollama model by name
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/model qwen2.5-coder:32b", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.model, "qwen2.5-coder:32b");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Switched active model to: qwen2.5-coder:32b (Ollama)"));

    // Switch to Gemini model by name
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/model gemini-2.5-pro", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.model, "gemini-2.5-pro");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Switched active model to: gemini-2.5-pro (Gemini)"));
}

#[test]
fn test_slash_model_invalid_numeric_index() {
    let mut session = create_test_session();
    session.cached_models = vec![ModelInfo {
        id: "gemini-3.8-flash".to_string(),
        display_name: "Gemini 3.8 Flash".to_string(),
        description: None,
        provider: "gemini".to_string(),
    }];

    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/model 999", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.model, "gemini-3.8-flash");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Invalid model index: 999. Use /model to view available models."));

    let mut out2 = Vec::new();
    let res2 = handle_repl_command(&mut session, "/model 0", &mut out2).unwrap();
    assert_eq!(res2, ReplControl::Handled);
    let text2 = String::from_utf8(out2).unwrap();
    assert!(text2.contains("Invalid model index: 0. Use /model to view available models."));
}

#[test]
fn test_slash_model_whitespace_padded_args() {
    let mut session = create_test_session();
    session.cached_models = vec![ModelInfo {
        id: "claude-3-7-sonnet-latest".to_string(),
        display_name: "Claude 3.7 Sonnet".to_string(),
        description: None,
        provider: "anthropic".to_string(),
    }];

    // Whitespace only behaves identically to empty args (discovery)
    let mut out1 = Vec::new();
    let res1 = handle_repl_command(&mut session, "/model    ", &mut out1).unwrap();
    assert_eq!(res1, ReplControl::Handled);
    let text1 = String::from_utf8(out1).unwrap();
    assert!(text1.contains("Discovering available cloud models..."));

    // Reset cache and test padded index
    session.cached_models = vec![ModelInfo {
        id: "claude-3-7-sonnet-latest".to_string(),
        display_name: "Claude 3.7 Sonnet".to_string(),
        description: None,
        provider: "anthropic".to_string(),
    }];
    let mut out2 = Vec::new();
    let res2 = handle_repl_command(&mut session, "/model   1   ", &mut out2).unwrap();
    assert_eq!(res2, ReplControl::Handled);
    assert_eq!(session.model, "claude-3-7-sonnet-latest");
    let text2 = String::from_utf8(out2).unwrap();
    assert!(text2.contains("Switched active model to: claude-3-7-sonnet-latest (anthropic)"));

    // Padded model name
    let mut out3 = Vec::new();
    let res3 = handle_repl_command(&mut session, "/model   gemini-2.5-pro   ", &mut out3).unwrap();
    assert_eq!(res3, ReplControl::Handled);
    assert_eq!(session.model, "gemini-2.5-pro");
    let text3 = String::from_utf8(out3).unwrap();
    assert!(text3.contains("Switched active model to: gemini-2.5-pro (Gemini)"));
}

#[test]
fn test_slash_model_negative_index_and_on_demand_cache() {
    let mut session = create_test_session();
    assert!(session.cached_models.is_empty());

    // Negative index reports invalid index rather than treating as model name
    let mut out1 = Vec::new();
    let res1 = handle_repl_command(&mut session, "/model -1", &mut out1).unwrap();
    assert_eq!(res1, ReplControl::Handled);
    let text1 = String::from_utf8(out1).unwrap();
    assert!(text1.contains("Invalid model index: -1. Use /model to view available models."));

    // Out of range index on empty cache triggers on-demand discovery
    let mut out2 = Vec::new();
    let res2 = handle_repl_command(&mut session, "/model 9999", &mut out2).unwrap();
    assert_eq!(res2, ReplControl::Handled);
    let text2 = String::from_utf8(out2).unwrap();
    assert!(text2.contains("Invalid model index: 9999. Use /model to view available models."));
}

#[test]
fn test_doctor_handshake_verification_includes_step_6() {
    let res = superstate_cli::commands::doctor::run_doctor();
    assert!(res.is_ok());
}
