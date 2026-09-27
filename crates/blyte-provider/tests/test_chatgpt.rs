//! The ChatGPT-login adapter: reuse a Codex CLI login (never refresh), speak the Responses API
//! over SSE. Recorded data only; a temporary CODEX_HOME, never the real one.

use blyte_provider::chatgpt::ChatGpt;
use blyte_provider::registry::{detect_provider_kind, ProviderKind};
use blyte_provider::{Message, ProviderError, Request, ToolSpec};
use serde_json::json;

fn b64url(s: &str) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let b = s.as_bytes();
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..(c.len() + 1) {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn jwt(exp: u64, account: &str) -> String {
    let payload =
        json!({"exp": exp, "https://api.openai.com/auth": {"chatgpt_account_id": account}});
    format!(
        "{}.{}.sig",
        b64url(r#"{"alg":"none"}"#),
        b64url(&payload.to_string())
    )
}

fn home(auth: serde_json::Value) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("auth.json"), auth.to_string()).unwrap();
    d
}

const NOW: u64 = 1_800_000_000;

#[test]
fn a_valid_login_is_reused_and_the_account_id_comes_from_the_token_when_absent() {
    let h =
        home(json!({"tokens": {"access_token": jwt(NOW + 3600, "acct-123"), "account_id": null}}));
    let p = ChatGpt::from_codex_home(h.path(), NOW)
        .unwrap()
        .with_model("gpt-5.5");
    assert_eq!(p.account_id(), "acct-123");
    assert_eq!(blyte_provider::Provider::model_id(&p), "chatgpt/gpt-5.5");
}

#[test]
fn a_token_about_to_expire_is_refused_with_how_to_refresh_it() {
    let h = home(json!({"tokens": {"access_token": jwt(NOW + 60, "a"), "account_id": "a"}}));
    match ChatGpt::from_codex_home(h.path(), NOW) {
        Err(ProviderError::Parse(m)) => {
            assert!(m.contains("run any codex command to refresh"), "{m}")
        }
        Err(e) => panic!("wrong error: {e:?}"),
        Ok(_) => panic!("an expiring token must be refused"),
    }
}

#[test]
fn a_missing_login_is_a_missing_key() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(
        ChatGpt::from_codex_home(d.path(), NOW),
        Err(ProviderError::MissingKey(_))
    ));
}

#[test]
fn the_body_is_a_stateless_streaming_responses_request() {
    let h = home(json!({"tokens": {"access_token": jwt(NOW + 3600, "a"), "account_id": "a"}}));
    let p = ChatGpt::from_codex_home(h.path(), NOW)
        .unwrap()
        .with_model("gpt-5.5");
    let b = p.body(&Request {
        system: String::new(),
        messages: vec![
            Message::User("fix it".into()),
            Message::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                arguments: json!({"path": "a"}),
                signature: None,
            },
            Message::ToolResult {
                id: "c1".into(),
                name: "read".into(),
                content: "A".into(),
                is_error: false,
            },
        ],
        tools: vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
            parameters: json!({"type": "object"}),
        }],
        max_output_tokens: None,
    });
    assert_eq!(b["store"], false);
    assert_eq!(b["stream"], true);
    assert_eq!(b["instructions"], "You are a helpful assistant.");
    let input = b["input"].as_array().unwrap();
    assert_eq!(input[0]["content"][0]["type"], "input_text");
    assert_eq!(input[1]["type"], "function_call");
    assert!(input[1]["arguments"].is_string());
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(input[2]["call_id"], "c1");
    assert_eq!(b["tools"][0]["type"], "function");
    assert_eq!(b["tools"][0]["name"], "read");
}

#[test]
fn sse_yields_tool_calls_text_and_usage() {
    let sse = [
        r#"data: {"type":"response.created","response":{}}"#,
        r#"data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"c9","name":"bash","arguments":"{\"command\":\"ls\"}"}}"#,
        r#"data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Looking."}]}}"#,
        r#"data: {"type":"response.completed","response":{"usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":600},"output_tokens":300,"output_tokens_details":{"reasoning_tokens":100}}}}"#,
        "",
    ].join("\n\n");
    let r = ChatGpt::parse_sse(&sse).unwrap();
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].id, "c9");
    assert_eq!(r.tool_calls[0].arguments, json!({"command": "ls"}));
    assert_eq!(r.text, "Looking.");
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
fn a_failed_response_is_an_error() {
    let sse = r#"data: {"type":"response.failed","response":{"error":{"message":"quota"}}}"#;
    assert!(ChatGpt::parse_sse(sse).is_err());
}

#[test]
fn the_prefix_routes_before_the_ollama_colon_rule() {
    assert_eq!(
        detect_provider_kind(Some("chatgpt/gpt-5.5")),
        ProviderKind::ChatGpt
    );
}
