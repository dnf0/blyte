//! The Anthropic Claude request/response mapping and model discovery, tested without a network.
//!
//! Wire translations are tested offline to verify formatting, role mappings, tool handling,
//! token usage accounting, and model listing without requiring live credentials or network access.

use std::sync::Mutex;

use serde_json::json;
use superstate_provider::anthropic::Anthropic;
use superstate_provider::{Message, Provider, ProviderError, Request, ToolSpec, Usage};

/// Lock to serialize tests modifying process-wide environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn anthropic() -> Anthropic {
    std::env::set_var("ANTHROPIC_API_KEY", "test-key-not-real");
    Anthropic::from_env().unwrap()
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "read_file".into(),
        description: "Read a file".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
    }
}

#[test]
fn system_instruction_is_placed_in_top_level_system_field() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        system: "You are an assistant.".into(),
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = anthropic().body(&req);
    assert_eq!(b["system"], "You are an assistant.");
    assert_eq!(
        b["messages"].as_array().unwrap().len(),
        1,
        "system prompt must not be included in messages"
    );
    assert_eq!(b["model"], "claude-3-7-sonnet-latest");
    assert_eq!(b["max_tokens"], 8192);
}

#[test]
fn empty_system_instruction_is_omitted() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        system: "".into(),
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = anthropic().body(&req);
    assert!(b.get("system").is_none());
}

#[test]
fn custom_max_output_tokens_is_respected() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        max_output_tokens: Some(4096),
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = anthropic().body(&req);
    assert_eq!(b["max_tokens"], 4096);
}

#[test]
fn user_and_assistant_message_content_blocks() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        messages: vec![
            Message::User("hello".into()),
            Message::Assistant("hi".into()),
        ],
        ..Default::default()
    };
    let b = anthropic().body(&req);
    assert_eq!(b["messages"][0]["role"], "user");
    assert_eq!(b["messages"][0]["content"][0]["type"], "text");
    assert_eq!(b["messages"][0]["content"][0]["text"], "hello");

    assert_eq!(b["messages"][1]["role"], "assistant");
    assert_eq!(b["messages"][1]["content"][0]["type"], "text");
    assert_eq!(b["messages"][1]["content"][0]["text"], "hi");
}

#[test]
fn tool_call_and_tool_result_wire_shape() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        messages: vec![
            Message::User("read file".into()),
            Message::ToolCall {
                id: "toolu_123".into(),
                name: "read_file".into(),
                arguments: json!({"path": "foo.rs"}),
                signature: None,
            },
            Message::ToolResult {
                id: "toolu_123".into(),
                name: "read_file".into(),
                content: "pub fn bar() {}".into(),
                is_error: false,
            },
        ],
        tools: vec![tool()],
        ..Default::default()
    };
    let b = anthropic().body(&req);

    // Assistant tool_use block
    assert_eq!(b["messages"][1]["role"], "assistant");
    assert_eq!(b["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(b["messages"][1]["content"][0]["id"], "toolu_123");
    assert_eq!(b["messages"][1]["content"][0]["name"], "read_file");
    assert_eq!(
        b["messages"][1]["content"][0]["input"],
        json!({"path": "foo.rs"})
    );

    // User tool_result block
    assert_eq!(b["messages"][2]["role"], "user");
    assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(b["messages"][2]["content"][0]["tool_use_id"], "toolu_123");
    assert_eq!(b["messages"][2]["content"][0]["content"], "pub fn bar() {}");
    assert_eq!(b["messages"][2]["content"][0]["is_error"], false);

    // Tools schema
    assert_eq!(b["tools"][0]["name"], "read_file");
    assert_eq!(b["tools"][0]["description"], "Read a file");
    assert_eq!(
        b["tools"][0]["input_schema"],
        json!({"type":"object","properties":{"path":{"type":"string"}}})
    );
}

#[test]
fn tools_omitted_when_empty() {
    let _guard = ENV_LOCK.lock().unwrap();
    let b = anthropic().body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert!(b.get("tools").is_none());
}

#[test]
fn parses_text_and_tool_use_response() {
    let v = json!({
        "id": "msg_01X9vhvWxAE3JeewndpTxWs5",
        "type": "message",
        "role": "assistant",
        "content": [
            {"type": "text", "text": "I will inspect that for you."},
            {"type": "tool_use", "id": "toolu_01A09q90tc1q", "name": "read_file", "input": {"path": "main.rs"}}
        ],
        "model": "claude-3-7-sonnet-20250219",
        "stop_reason": "tool_use",
        "usage": {
            "input_tokens": 1200,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 400,
            "output_tokens": 80
        }
    });
    let r = Anthropic::parse(&v).expect("parsing response should succeed");
    assert_eq!(r.text, "I will inspect that for you.");
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].id, "toolu_01A09q90tc1q");
    assert_eq!(r.tool_calls[0].name, "read_file");
    assert_eq!(r.tool_calls[0].arguments["path"], "main.rs");
    assert_eq!(r.tool_calls[0].signature, None);
    assert_eq!(r.finish_reason.as_deref(), Some("tool_use"));
    assert_eq!(
        r.usage,
        Usage {
            input: 1200,
            cached: 400,
            output: 80,
            thinking: 0,
        }
    );
}

#[test]
fn consecutive_assistant_messages_coalesced_into_single_turn() {
    let _guard = ENV_LOCK.lock().unwrap();
    let req = Request {
        messages: vec![
            Message::User("inspect file".into()),
            Message::Assistant("I am inspecting the file now.".into()),
            Message::ToolCall {
                id: "toolu_456".into(),
                name: "read_file".into(),
                arguments: json!({"path": "lib.rs"}),
                signature: None,
            },
        ],
        ..Default::default()
    };
    let b = anthropic().body(&req);
    let msgs = b["messages"].as_array().unwrap();
    // Must coalesce into exactly 2 turns: 1 user, 1 assistant (with 2 content blocks)
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[1]["role"], "assistant");
    let content = msgs[1]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "I am inspecting the file now.");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(content[1]["id"], "toolu_456");
}

#[test]
fn response_with_no_usage_block_does_not_fail() {
    let v = json!({
        "content": [{"type": "text", "text": "hello"}]
    });
    let r = Anthropic::parse(&v).expect("should parse successfully");
    assert_eq!(r.text, "hello");
    assert_eq!(r.usage, Usage::default());
}

#[test]
fn response_without_content_fails_parsing() {
    let v = json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "message": "max_tokens: 100000 > 8192"
        }
    });
    let err = Anthropic::parse(&v);
    assert!(
        err.is_err(),
        "response without a content array must fail parsing"
    );
}

#[test]
fn parse_models_list_extracts_models_and_adds_standard_aliases() {
    let v = json!({
        "data": [
            {
                "type": "model",
                "id": "claude-3-5-sonnet-20241022",
                "display_name": "Claude 3.5 Sonnet",
                "created_at": "2024-10-22T00:00:00Z"
            }
        ]
    });
    let models = Anthropic::parse_models_list(&v).expect("parsing models list should succeed");
    // Contains "claude-3-5-sonnet-20241022" plus the 3 standard aliases ("claude-3-7-sonnet-latest", "claude-3-5-sonnet-latest", "claude-3-5-haiku-latest").
    assert_eq!(models.len(), 4);
    assert!(models.iter().any(|m| m.id == "claude-3-5-sonnet-20241022"
        && m.display_name == "Claude 3.5 Sonnet"
        && m.provider == "anthropic"));
    assert!(models
        .iter()
        .any(|m| m.id == "claude-3-7-sonnet-latest" && m.provider == "anthropic"));
    assert!(models
        .iter()
        .any(|m| m.id == "claude-3-5-sonnet-latest" && m.provider == "anthropic"));
    assert!(models
        .iter()
        .any(|m| m.id == "claude-3-5-haiku-latest" && m.provider == "anthropic"));
}

#[test]
fn parse_models_list_handles_empty_or_missing_data() {
    let empty = json!({"data": []});
    let models = Anthropic::parse_models_list(&empty).expect("empty data should succeed");
    assert_eq!(models.len(), 3);

    let missing = json!({});
    let models = Anthropic::parse_models_list(&missing).expect("missing data should succeed");
    assert_eq!(models.len(), 3);
}

#[test]
fn from_env_requires_api_key_and_honors_superstate_model() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("ANTHROPIC_API_KEY");
    assert!(matches!(
        Anthropic::from_env(),
        Err(ProviderError::MissingKey("ANTHROPIC_API_KEY"))
    ));

    std::env::set_var("ANTHROPIC_API_KEY", "test-key");
    std::env::set_var("SUPERSTATE_MODEL", "claude-3-5-sonnet-20241022");
    let a = Anthropic::from_env().expect("from_env should succeed when key is set");
    assert_eq!(a.model_id(), "anthropic/claude-3-5-sonnet-20241022");

    std::env::remove_var("SUPERSTATE_MODEL");
    let a_default =
        Anthropic::from_env().expect("from_env should default to claude-3-7-sonnet-latest");
    assert_eq!(a_default.model_id(), "anthropic/claude-3-7-sonnet-latest");
}

#[test]
fn with_model_updates_model_and_model_id() {
    let _guard = ENV_LOCK.lock().unwrap();
    let a = anthropic().with_model("claude-opus-4");
    assert_eq!(a.model_id(), "anthropic/claude-opus-4");
}
