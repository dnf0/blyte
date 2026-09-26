//! The OpenAI-compatible adapter: wire shape, response parsing and routing. Recorded JSON only.

use std::sync::Mutex;

use blyte_provider::openai::OpenAi;
use blyte_provider::registry::{detect_provider_kind, ProviderKind};
use blyte_provider::{resolve_provider, Message, ProviderError, Request, ToolSpec};
use serde_json::json;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn clear_env() {
    for k in [
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "ANTHROPIC_API_KEY",
        "OLLAMA_API_KEY",
        "OLLAMA_HOST",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENROUTER_API_KEY",
        "BLYTE_MODEL",
    ] {
        std::env::remove_var(k);
    }
}

fn provider() -> OpenAi {
    let _g = ENV_LOCK.lock().unwrap();
    clear_env();
    std::env::set_var("OPENAI_API_KEY", "test-key-not-real");
    let p = OpenAi::openai_from_env().unwrap().with_model("gpt-5.5");
    clear_env();
    p
}

#[test]
fn the_body_puts_system_first_and_merges_a_parallel_batch() {
    let req = Request {
        system: "be brief".into(),
        messages: vec![
            Message::User("fix it".into()),
            Message::ToolCall {
                id: "a".into(),
                name: "read".into(),
                arguments: json!({"path": "x"}),
                signature: None,
            },
            Message::ToolCall {
                id: "b".into(),
                name: "bash".into(),
                arguments: json!({"command": "ls"}),
                signature: None,
            },
            Message::ToolResult {
                id: "a".into(),
                name: "read".into(),
                content: "X".into(),
                is_error: false,
            },
            Message::ToolResult {
                id: "b".into(),
                name: "bash".into(),
                content: "Y".into(),
                is_error: false,
            },
        ],
        tools: vec![ToolSpec {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object"}),
        }],
        max_output_tokens: None,
    };
    let b = provider().body(&req);
    assert_eq!(b["model"], "gpt-5.5");
    let m = b["messages"].as_array().unwrap();
    assert_eq!(
        m.len(),
        5,
        "system, user, ONE assistant with both calls, two tool results: {m:?}"
    );
    assert_eq!(m[0]["role"], "system");
    assert_eq!(m[2]["role"], "assistant");
    let calls = m[2]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["function"]["name"], "read");
    assert!(
        calls[0]["function"]["arguments"].is_string(),
        "arguments go over the wire as a JSON string"
    );
    assert_eq!(m[3]["role"], "tool");
    assert_eq!(m[3]["tool_call_id"], "a");
    assert_eq!(b["tools"][0]["function"]["name"], "read");
    assert!(b.get("max_completion_tokens").is_none());
}

#[test]
fn text_and_usage_are_parsed_with_cache_and_reasoning_split_out() {
    let r = OpenAi::parse(&json!({
        "choices": [{"message": {"role": "assistant", "content": "done"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1000, "completion_tokens": 300,
                  "prompt_tokens_details": {"cached_tokens": 600},
                  "completion_tokens_details": {"reasoning_tokens": 100}}
    }))
    .unwrap();
    assert_eq!(r.text, "done");
    assert_eq!(r.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        (
            r.usage.input,
            r.usage.cached,
            r.usage.output,
            r.usage.thinking
        ),
        (400, 600, 200, 100)
    );
}

#[test]
fn tool_calls_are_parsed_from_json_string_arguments() {
    let r = OpenAi::parse(&json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a.rs\"}"}},
            {"id": "c2", "type": "function", "function": {"name": "bash", "arguments": "not json"}}
        ]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    })).unwrap();
    assert_eq!(r.text, "");
    assert_eq!(r.tool_calls.len(), 2);
    assert_eq!(r.tool_calls[0].id, "c1");
    assert_eq!(r.tool_calls[0].arguments, json!({"path": "a.rs"}));
    assert_eq!(r.tool_calls[1].arguments, json!({}));
}

#[test]
fn an_error_payload_is_a_parse_error() {
    assert!(matches!(
        OpenAi::parse(&json!({"error": {"message": "bad model", "type": "invalid_request_error"}})),
        Err(ProviderError::Parse(_))
    ));
}

#[test]
fn a_local_server_needs_only_a_base_url() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_env();
    assert!(matches!(
        OpenAi::openai_from_env(),
        Err(ProviderError::MissingKey("OPENAI_API_KEY"))
    ));
    std::env::set_var("OPENAI_BASE_URL", "http://localhost:8000/v1/");
    assert!(OpenAi::openai_from_env().is_ok());
    clear_env();
    assert!(matches!(
        OpenAi::openrouter_from_env(),
        Err(ProviderError::MissingKey("OPENROUTER_API_KEY"))
    ));
}

#[test]
fn prefixes_route_before_the_ollama_colon_rule() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_env();
    assert_eq!(
        detect_provider_kind(Some("openai/gpt-5.5")),
        ProviderKind::OpenAi
    );
    assert_eq!(
        detect_provider_kind(Some("openrouter/deepseek/deepseek-r1:free")),
        ProviderKind::OpenRouter
    );
    assert_eq!(
        detect_provider_kind(Some("qwen3:14b")),
        ProviderKind::Ollama
    );

    std::env::set_var("OPENROUTER_API_KEY", "test-key-not-real");
    let p = resolve_provider(Some("openrouter/meta/muse-spark-1.3")).unwrap();
    assert_eq!(p.model_id(), "openrouter/meta/muse-spark-1.3");
    std::env::set_var("OPENAI_API_KEY", "test-key-not-real");
    let p = resolve_provider(Some("openai/gpt-5.5")).unwrap();
    assert_eq!(p.model_id(), "openai/gpt-5.5");
    clear_env();
}
