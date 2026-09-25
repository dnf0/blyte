//! The Ollama Cloud / custom host request/response wire mapping and model discovery, tested without a network.
//!
//! Wire translations are tested offline to verify formatting, role mappings, tool handling,
//! token usage accounting, and model listing without requiring live credentials or network access.

use std::sync::Mutex;

use blyte_provider::ollama::Ollama;
use blyte_provider::{Message, Provider, ProviderError, Request, ToolSpec, Usage};
use serde_json::json;

/// Lock to serialize tests modifying process-wide environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn ollama_cloud() -> Ollama {
    std::env::set_var("OLLAMA_API_KEY", "test-key-not-real");
    std::env::remove_var("OLLAMA_HOST");
    std::env::remove_var("BLYTE_MODEL");
    Ollama::from_env().unwrap()
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "bash".into(),
        description: "Run a bash command".into(),
        parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
    }
}

#[test]
fn system_instruction_is_placed_in_messages_array_when_present() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        system: "You are a helpful coding assistant.".into(),
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = ollama_cloud().body(&req);
    let msgs = b["messages"].as_array().expect("messages must be an array");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], "You are a helpful coding assistant.");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(msgs[1]["content"], "hi");
    assert_eq!(b["model"], "qwen2.5-coder:32b");
    assert_eq!(b["stream"], false);
}

#[test]
fn empty_system_instruction_is_omitted_from_messages() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        system: "   ".into(),
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = ollama_cloud().body(&req);
    let msgs = b["messages"].as_array().expect("messages must be an array");
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "hi");
}

#[test]
fn user_and_assistant_messages_wire_formatting() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        messages: vec![
            Message::User("hello".into()),
            Message::Assistant("world".into()),
        ],
        ..Default::default()
    };
    let b = ollama_cloud().body(&req);
    let msgs = b["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "hello");
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"], "world");
}

#[test]
fn tool_call_and_tool_result_wire_shape() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        messages: vec![
            Message::User("run ls".into()),
            Message::ToolCall {
                id: "call_123".into(),
                name: "bash".into(),
                arguments: json!({"cmd": "ls -la"}),
                signature: None,
            },
            Message::ToolResult {
                id: "call_123".into(),
                name: "bash".into(),
                content: "Cargo.toml\nsrc\n".into(),
                is_error: false,
            },
        ],
        tools: vec![tool()],
        ..Default::default()
    };
    let b = ollama_cloud().body(&req);

    // Assistant tool_calls message
    let msgs = b["messages"].as_array().unwrap();
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"], "");
    let tool_calls = msgs[1]["tool_calls"].as_array().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["id"], "call_123");
    assert_eq!(tool_calls[0]["type"], "function");
    assert_eq!(tool_calls[0]["function"]["name"], "bash");
    assert_eq!(
        tool_calls[0]["function"]["arguments"],
        json!({"cmd": "ls -la"})
    );

    // Tool result message
    assert_eq!(msgs[2]["role"], "tool");
    assert_eq!(msgs[2]["tool_call_id"], "call_123");
    assert_eq!(msgs[2]["content"], "Cargo.toml\nsrc\n");

    // Tools schema
    let tools = b["tools"].as_array().expect("tools must be present");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "bash");
    assert_eq!(tools[0]["function"]["description"], "Run a bash command");
    assert_eq!(
        tools[0]["function"]["parameters"],
        json!({"type":"object","properties":{"cmd":{"type":"string"}}})
    );
}

#[test]
fn tools_omitted_when_empty() {
    let _guard = ENV_LOCK.lock().unwrap();
    let b = ollama_cloud().body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert!(b.get("tools").is_none());
}

#[test]
fn parses_text_response_and_usage() {
    let v = json!({
        "model": "qwen2.5-coder:32b",
        "created_at": "2026-09-23T10:00:00Z",
        "message": {
            "role": "assistant",
            "content": "Hello! I can help you code."
        },
        "done": true,
        "done_reason": "stop",
        "prompt_eval_count": 42,
        "eval_count": 18
    });
    let r = Ollama::parse(&v).expect("parsing response should succeed");
    assert_eq!(r.text, "Hello! I can help you code.");
    assert!(r.tool_calls.is_empty());
    assert_eq!(r.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        r.usage,
        Usage {
            input: 42,
            cached: 0,
            output: 18,
            thinking: 0,
        }
    );
}

#[test]
fn parses_tool_calls_with_object_arguments() {
    let v = json!({
        "model": "qwen2.5-coder:32b",
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_abc",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": {
                            "cmd": "git status"
                        }
                    }
                }
            ]
        },
        "done": true,
        "done_reason": "stop"
    });
    let r = Ollama::parse(&v).expect("should parse tool calls");
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].id, "call_abc");
    assert_eq!(r.tool_calls[0].name, "bash");
    assert_eq!(r.tool_calls[0].arguments, json!({"cmd": "git status"}));
    assert_eq!(r.tool_calls[0].signature, None);
}

#[test]
fn parses_tool_calls_with_string_arguments_and_fallback_id() {
    let v = json!({
        "model": "qwen2.5-coder:32b",
        "message": {
            "role": "assistant",
            "content": "Running command...",
            "tool_calls": [
                {
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":\"cargo test\"}"
                    }
                }
            ]
        },
        "done": true,
        "done_reason": "stop"
    });
    let r = Ollama::parse(&v).expect("should parse string-encoded arguments");
    assert_eq!(r.text, "Running command...");
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].id, "call_0");
    assert_eq!(r.tool_calls[0].name, "bash");
    assert_eq!(r.tool_calls[0].arguments, json!({"cmd": "cargo test"}));
}

#[test]
fn parses_error_response_fails() {
    let v = json!({
        "error": "model 'nonexistent-model' not found"
    });
    let err = Ollama::parse(&v);
    assert!(err.is_err(), "response with error field must fail parsing");
}

#[test]
fn parse_models_list_extracts_models_and_details() {
    let v = json!({
        "models": [
            {
                "name": "qwen2.5-coder:32b",
                "model": "qwen2.5-coder:32b",
                "modified_at": "2026-09-20T00:00:00Z",
                "size": 19284093,
                "details": {
                    "format": "gguf",
                    "family": "qwen2",
                    "parameter_size": "32B",
                    "quantization_level": "Q4_K_M"
                }
            },
            {
                "name": "llama3.3:70b",
                "model": "llama3.3:70b",
                "details": {
                    "parameter_size": "70B"
                }
            }
        ]
    });
    let models = Ollama::parse_models_list(&v).expect("parsing models list should succeed");
    assert_eq!(models.len(), 2);

    assert_eq!(models[0].id, "qwen2.5-coder:32b");
    assert_eq!(models[0].display_name, "qwen2.5-coder:32b");
    assert_eq!(models[0].provider, "ollama");
    assert_eq!(models[0].description.as_deref(), Some("32B, Q4_K_M"));

    assert_eq!(models[1].id, "llama3.3:70b");
    assert_eq!(models[1].display_name, "llama3.3:70b");
    assert_eq!(models[1].provider, "ollama");
    assert_eq!(models[1].description.as_deref(), Some("70B"));
}

#[test]
fn parse_models_list_handles_empty_or_missing_models() {
    let empty = json!({"models": []});
    let models = Ollama::parse_models_list(&empty).expect("empty models array should succeed");
    assert!(models.is_empty());

    let missing = json!({});
    let models = Ollama::parse_models_list(&missing).expect("missing models key should succeed");
    assert!(models.is_empty());
}

#[test]
fn from_env_requires_api_key_or_host() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("OLLAMA_HOST");
    std::env::remove_var("BLYTE_MODEL");
    assert!(matches!(
        Ollama::from_env(),
        Err(ProviderError::MissingKey("OLLAMA_API_KEY"))
    ));

    std::env::set_var("OLLAMA_API_KEY", "test-key");
    let o = Ollama::from_env().expect("succeeds with API key");
    assert_eq!(o.api_root(), "https://ollama.com/api");
    assert_eq!(o.api_key(), Some("test-key"));
    assert_eq!(o.model_id(), "ollama/qwen2.5-coder:32b");

    std::env::remove_var("OLLAMA_API_KEY");
    std::env::set_var("OLLAMA_HOST", "http://localhost:11434/");
    let o2 = Ollama::from_env().expect("succeeds with host even if key unset");
    assert_eq!(o2.api_root(), "http://localhost:11434/api");
    assert_eq!(o2.api_key(), None);

    std::env::remove_var("OLLAMA_HOST");
}

#[test]
fn from_env_honors_blyte_model() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("OLLAMA_API_KEY", "test-key");
    std::env::set_var("BLYTE_MODEL", "deepseek-coder:33b");
    let o = Ollama::from_env().expect("from_env should succeed");
    assert_eq!(o.model_id(), "ollama/deepseek-coder:33b");

    std::env::remove_var("BLYTE_MODEL");
    let o_default = Ollama::from_env().expect("from_env defaults to qwen2.5-coder:32b");
    assert_eq!(o_default.model_id(), "ollama/qwen2.5-coder:32b");
}

#[test]
fn builder_methods_work() {
    let _guard = ENV_LOCK.lock().unwrap();
    let o = ollama_cloud()
        .with_model("llama3.1:8b")
        .with_api_root("https://custom.ollama.local/api/")
        .with_api_key("custom-key");
    assert_eq!(o.model_id(), "ollama/llama3.1:8b");
    assert_eq!(o.api_root(), "https://custom.ollama.local/api");
    assert_eq!(o.api_key(), Some("custom-key"));
}
