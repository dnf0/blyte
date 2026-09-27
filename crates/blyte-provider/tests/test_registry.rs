//! Tests for universal provider registry, routing, and discovery.
//!
//! Validates model identifier prefix detection, ambient environment credential fallback,
//! provider instantiation, and multi-provider discovery without requiring network calls.

use blyte_provider::registry::{detect_provider_kind, ProviderKind};
use blyte_provider::{discover_all_models, resolve_provider, ProviderError};
use std::sync::Mutex;

/// Mutex to serialize tests that mutate process environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Helper to clear all provider-related environment variables for isolation.
fn clear_provider_env() {
    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("OLLAMA_HOST");
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENROUTER_API_KEY");
    std::env::remove_var("BLYTE_MODEL");
    std::env::remove_var("CODEX_HOME");
}

#[test]
fn test_detect_provider_kind_by_model_prefix_and_syntax() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_provider_env();

    // Anthropic routing: claude prefix or anthropic/ namespace
    assert_eq!(
        detect_provider_kind(Some("claude-3-7-sonnet")),
        ProviderKind::Anthropic
    );
    assert_eq!(
        detect_provider_kind(Some("claude-3-5-haiku")),
        ProviderKind::Anthropic
    );
    assert_eq!(
        detect_provider_kind(Some("anthropic/claude-3-5-haiku")),
        ProviderKind::Anthropic
    );

    // Gemini routing: gemini prefix or gemini/ namespace
    assert_eq!(
        detect_provider_kind(Some("gemini-3.8-flash")),
        ProviderKind::Gemini
    );
    assert_eq!(
        detect_provider_kind(Some("gemini-2.5-pro")),
        ProviderKind::Gemini
    );
    assert_eq!(
        detect_provider_kind(Some("gemini/gemini-2.5-pro")),
        ProviderKind::Gemini
    );

    // Ollama routing: colon tag notation or ollama/ namespace or literal "ollama"
    assert_eq!(
        detect_provider_kind(Some("qwen2.5-coder:32b")),
        ProviderKind::Ollama
    );
    assert_eq!(
        detect_provider_kind(Some("llama3.1:8b")),
        ProviderKind::Ollama
    );
    assert_eq!(
        detect_provider_kind(Some("ollama/llama3.3:70b")),
        ProviderKind::Ollama
    );
    assert_eq!(detect_provider_kind(Some("ollama")), ProviderKind::Ollama);
}

#[test]
fn test_detect_provider_kind_fallback_to_environment_credentials() {
    let _guard = ENV_LOCK.lock().unwrap();

    // 1. With GEMINI_API_KEY set, Gemini is preferred for unspecified models
    clear_provider_env();
    std::env::set_var("GEMINI_API_KEY", "mock-gemini-key");
    assert_eq!(detect_provider_kind(None), ProviderKind::Gemini);
    assert_eq!(
        detect_provider_kind(Some("custom-model")),
        ProviderKind::Gemini
    );

    // 2. With GOOGLE_API_KEY set, Gemini is preferred
    clear_provider_env();
    std::env::set_var("GOOGLE_API_KEY", "mock-google-key");
    assert_eq!(detect_provider_kind(None), ProviderKind::Gemini);

    // 3. With only ANTHROPIC_API_KEY set, Anthropic is preferred
    clear_provider_env();
    std::env::set_var("ANTHROPIC_API_KEY", "mock-anthropic-key");
    assert_eq!(detect_provider_kind(None), ProviderKind::Anthropic);
    assert_eq!(
        detect_provider_kind(Some("custom-model")),
        ProviderKind::Anthropic
    );

    // 4. With only OLLAMA_API_KEY or OLLAMA_HOST set, Ollama is preferred
    clear_provider_env();
    std::env::set_var("OLLAMA_API_KEY", "mock-ollama-key");
    assert_eq!(detect_provider_kind(None), ProviderKind::Ollama);

    clear_provider_env();
    std::env::set_var("OLLAMA_HOST", "http://localhost:11434");
    assert_eq!(detect_provider_kind(None), ProviderKind::Ollama);

    // 5. Default fallback when no credentials exist in environment is Gemini
    clear_provider_env();
    assert_eq!(detect_provider_kind(None), ProviderKind::Gemini);
}

#[test]
fn test_resolve_provider_missing_api_keys() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_provider_env();

    // When keys are missing, resolve_provider propagates the provider-specific MissingKey error
    match resolve_provider(Some("anthropic/claude-3-7-sonnet")) {
        Err(ProviderError::MissingKey(key)) => assert_eq!(key, "ANTHROPIC_API_KEY"),
        Ok(_) => panic!("expected MissingKey(ANTHROPIC_API_KEY), got Ok"),
        Err(e) => panic!("expected MissingKey(ANTHROPIC_API_KEY), got other error: {e}"),
    }

    match resolve_provider(Some("gemini/gemini-3.8-flash")) {
        Err(ProviderError::MissingKey(key)) => assert_eq!(key, "GEMINI_API_KEY"),
        Ok(_) => panic!("expected MissingKey(GEMINI_API_KEY), got Ok"),
        Err(e) => panic!("expected MissingKey(GEMINI_API_KEY), got other error: {e}"),
    }

    match resolve_provider(Some("ollama/qwen2.5-coder:32b")) {
        Err(ProviderError::MissingKey(key)) => assert_eq!(key, "OLLAMA_API_KEY"),
        Ok(_) => panic!("expected MissingKey(OLLAMA_API_KEY), got Ok"),
        Err(e) => panic!("expected MissingKey(OLLAMA_API_KEY), got other error: {e}"),
    }

    // Default with no keys returns Gemini MissingKey
    match resolve_provider(None) {
        Err(ProviderError::MissingKey(key)) => assert_eq!(key, "GEMINI_API_KEY"),
        Ok(_) => panic!("expected MissingKey(GEMINI_API_KEY), got Ok"),
        Err(e) => panic!("expected MissingKey(GEMINI_API_KEY), got other error: {e}"),
    }
}

#[test]
fn test_resolve_provider_with_credentials_and_prefix_stripping() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_provider_env();
    std::env::set_var("ANTHROPIC_API_KEY", "mock-key");
    std::env::set_var("GEMINI_API_KEY", "mock-key");
    std::env::set_var("OLLAMA_API_KEY", "mock-key");

    // Strips anthropic/ prefix and configures Anthropic provider
    let anthropic = resolve_provider(Some("anthropic/claude-3-7-sonnet")).unwrap();
    assert_eq!(anthropic.model_id(), "anthropic/claude-3-7-sonnet");

    // Strips gemini/ prefix and configures Gemini provider
    let gemini = resolve_provider(Some("gemini/gemini-3.8-flash")).unwrap();
    assert_eq!(gemini.model_id(), "gemini/gemini-3.8-flash");

    // Strips ollama/ prefix and configures Ollama provider
    let ollama = resolve_provider(Some("ollama/qwen2.5-coder:32b")).unwrap();
    assert_eq!(ollama.model_id(), "ollama/qwen2.5-coder:32b");

    // Bare model names retain full identifier
    let bare_claude = resolve_provider(Some("claude-3-5-haiku")).unwrap();
    assert_eq!(bare_claude.model_id(), "anthropic/claude-3-5-haiku");

    let bare_ollama = resolve_provider(Some("llama3.3:70b")).unwrap();
    assert_eq!(bare_ollama.model_id(), "ollama/llama3.3:70b");
}

#[test]
fn test_discover_all_models_enumerates_providers() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_provider_env();
    let temp_codex = tempfile::tempdir().unwrap();
    std::env::set_var("CODEX_HOME", temp_codex.path());

    // Without credentials, discovery should report all 6 providers with missing key errors
    let results = discover_all_models();
    assert_eq!(results.len(), 6);

    let provider_names: Vec<&str> = results.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        provider_names,
        vec![
            "Google Gemini",
            "Anthropic Claude",
            "Ollama Cloud",
            "OpenAI",
            "OpenRouter",
            "ChatGPT",
        ]
    );

    for (name, res) in results {
        match res {
            Err(ProviderError::MissingKey(_)) => {}
            other => panic!("expected MissingKey for {name}, got: {other:?}"),
        }
    }

    clear_provider_env();
}
