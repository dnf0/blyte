//! Anthropic caches only what a request marks. The adapter marks the last tool and the newest
//! message block, so each turn reads the previous turn's prefix from cache; cache writes count.

use blyte_provider::anthropic::Anthropic;
use blyte_provider::{Message, Request, ToolSpec};
use serde_json::json;

fn anthropic() -> Anthropic {
    Anthropic::new("test-key-not-real").with_model("claude-sonnet-5")
}

fn tool(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "d".into(),
        parameters: json!({"type": "object"}),
    }
}

fn req(messages: Vec<Message>) -> Request {
    Request {
        system: "sys".into(),
        messages,
        tools: vec![tool("read"), tool("bash")],
        max_output_tokens: None,
    }
}

const EPHEMERAL: &str = "ephemeral";

#[test]
fn the_last_tool_is_a_cache_breakpoint() {
    let b = anthropic().body(&req(vec![Message::User("hi".into())]));
    assert!(b["tools"][0].get("cache_control").is_none());
    assert_eq!(b["tools"][1]["cache_control"]["type"], EPHEMERAL);
}

#[test]
fn the_newest_message_block_is_a_cache_breakpoint() {
    let b = anthropic().body(&req(vec![
        Message::User("fix it".into()),
        Message::ToolCall {
            id: "t1".into(),
            name: "read".into(),
            arguments: json!({"path": "a"}),
            signature: None,
        },
        Message::ToolResult {
            id: "t1".into(),
            name: "read".into(),
            content: "A".into(),
            is_error: false,
        },
    ]));
    let msgs = b["messages"].as_array().unwrap();
    let last = msgs.last().unwrap()["content"].as_array().unwrap();
    assert_eq!(last.last().unwrap()["cache_control"]["type"], EPHEMERAL);
    // Exactly two breakpoints in the whole body: the last tool and the newest block.
    assert_eq!(b.to_string().matches("\"cache_control\"").count(), 2, "{b}");
}

#[test]
fn an_empty_conversation_still_marks_the_tools() {
    let b = anthropic().body(&Request {
        system: "sys".into(),
        messages: vec![],
        tools: vec![tool("read")],
        max_output_tokens: None,
    });
    assert_eq!(b["tools"][0]["cache_control"]["type"], EPHEMERAL);
}

#[test]
fn cache_writes_count_as_input() {
    let r = Anthropic::parse(&json!({
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 50, "cache_creation_input_tokens": 3000,
                  "cache_read_input_tokens": 12000, "output_tokens": 20}
    }))
    .unwrap();
    assert_eq!(
        r.usage.input, 3050,
        "fresh input includes the tokens written to the cache"
    );
    assert_eq!(r.usage.cached, 12000);
    assert_eq!(r.usage.output, 20);
}
