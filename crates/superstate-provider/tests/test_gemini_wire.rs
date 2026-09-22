//! The request/response mapping, tested without a network.
//!
//! Every provider bug this project will hit lives in the translation, not the POST. These run
//! offline so they stay in the normal test loop rather than being skipped for want of a key.

use serde_json::json;
use superstate_provider::gemini::Gemini;
use superstate_provider::{Message, Request, ToolSpec, Usage};

fn gemini() -> Gemini {
    std::env::set_var("GEMINI_API_KEY", "test-key-not-real");
    Gemini::from_env().unwrap()
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "read_file".into(),
        description: "Read a file".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
    }
}

#[test]
fn system_instruction_is_separate_from_the_conversation() {
    // Gemini takes it as its own field. Prepending it as a user turn -- the obvious shortcut --
    // would make it uncacheable and would put it in the wrong role.
    let req = Request {
        system: "You are an engineer.".into(),
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = gemini().body(&req);
    assert_eq!(
        b["systemInstruction"]["parts"][0]["text"],
        "You are an engineer."
    );
    assert_eq!(
        b["contents"].as_array().unwrap().len(),
        1,
        "system must not become a turn"
    );
}

#[test]
fn assistant_turns_use_the_model_role() {
    let req = Request {
        messages: vec![Message::User("a".into()), Message::Assistant("b".into())],
        ..Default::default()
    };
    let b = gemini().body(&req);
    assert_eq!(b["contents"][0]["role"], "user");
    assert_eq!(
        b["contents"][1]["role"], "model",
        "Gemini says `model`, not `assistant`"
    );
}

#[test]
fn a_tool_call_round_trip_has_the_shape_gemini_expects() {
    // The call is a `model` turn carrying functionCall; the result is a `user` turn carrying
    // functionResponse. Getting either role wrong makes the model re-issue the same call.
    let req = Request {
        messages: vec![
            Message::User("read it".into()),
            Message::ToolCall {
                id: "call_0".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
                signature: Some("sig-abc".into()),
            },
            Message::ToolResult {
                id: "call_0".into(),
                name: "read_file".into(),
                content: "fn main(){}".into(),
                is_error: false,
            },
        ],
        tools: vec![tool()],
        ..Default::default()
    };
    let b = gemini().body(&req);

    assert_eq!(b["contents"][1]["role"], "model");
    assert_eq!(
        b["contents"][1]["parts"][0]["functionCall"]["name"],
        "read_file"
    );
    assert_eq!(
        b["contents"][1]["parts"][0]["functionCall"]["args"]["path"],
        "a.rs"
    );
    // Gemini 3.x rejects a replayed functionCall whose thoughtSignature is missing, with
    // INVALID_ARGUMENT rather than a quiet degradation. Found by running the loop, not by
    // reading the schema.
    assert_eq!(b["contents"][1]["parts"][0]["thoughtSignature"], "sig-abc");

    assert_eq!(b["contents"][2]["role"], "user");
    let fr = &b["contents"][2]["parts"][0]["functionResponse"];
    assert_eq!(fr["name"], "read_file");
    assert_eq!(fr["response"]["output"], "fn main(){}");
    assert_eq!(fr["response"]["error"], false);

    assert_eq!(
        b["tools"][0]["functionDeclarations"][0]["name"],
        "read_file"
    );
}

#[test]
fn tools_are_omitted_entirely_when_there_are_none() {
    // An empty `tools` array is not the same as no tools; some providers reject it.
    let b = gemini().body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert!(b.get("tools").is_none());
}

#[test]
fn parses_text_and_tool_calls_from_one_response() {
    let v = json!({"candidates":[{"content":{"parts":[
        {"text":"I'll read it."},
        {"functionCall":{"name":"read_file","args":{"path":"a.rs"}},
         "thoughtSignature":"sig-xyz"}
    ]},"finishReason":"STOP"}]});
    let r = Gemini::parse(&v).unwrap();
    assert_eq!(r.text, "I'll read it.");
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].name, "read_file");
    assert_eq!(r.tool_calls[0].arguments["path"], "a.rs");
    assert_eq!(
        r.tool_calls[0].signature.as_deref(),
        Some("sig-xyz"),
        "the signature must be captured or the next turn is rejected"
    );
    assert_eq!(r.finish_reason.as_deref(), Some("STOP"));
}

#[test]
fn cached_tokens_are_subtracted_from_input_not_double_counted() {
    // promptTokenCount INCLUDES cached tokens. Reporting it raw would say a well-cached turn
    // cost as much as a cold one -- and distinguishing those is the whole point of the
    // harness. EXP-081 saw 1,384,188 fresh against 15,327,627 cached in a single turn.
    let v = json!({"candidates":[{"content":{"parts":[{"text":"ok"}]}}],
        "usageMetadata":{"promptTokenCount":10_000,"cachedContentTokenCount":9_000,
                         "candidatesTokenCount":50,"thoughtsTokenCount":20}});
    let r = Gemini::parse(&v).unwrap();
    assert_eq!(
        r.usage,
        Usage {
            input: 1_000,
            cached: 9_000,
            output: 50,
            thinking: 20
        }
    );
}

#[test]
fn a_response_with_no_usage_block_does_not_fail() {
    let v = json!({"candidates":[{"content":{"parts":[{"text":"ok"}]}}]});
    assert_eq!(Gemini::parse(&v).unwrap().usage, Usage::default());
}

#[test]
fn a_candidateless_response_is_an_error_not_a_silent_empty() {
    // Safety blocks and quota errors arrive shaped like successes. Returning an empty response
    // would look to the loop like a model that chose to say nothing.
    let err = Gemini::parse(&json!({"promptFeedback":{"blockReason":"SAFETY"}}));
    assert!(
        err.is_err(),
        "a response with no candidates must surface, not parse to empty"
    );
}
