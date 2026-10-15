//! The request/response mapping, tested without a network.
//!
//! Every provider bug this project will hit lives in the translation, not the POST. These run
//! offline so they stay in the normal test loop rather than being skipped for want of a key.

use blyte_provider::gemini::Gemini;
use blyte_provider::{Message, Request, ToolSpec, Usage};
use serde_json::json;

fn gemini() -> Gemini {
    std::env::set_var("GEMINI_API_KEY", "test-key-not-real");
    std::env::remove_var("BLYTE_MODEL");
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
    let err = Gemini::parse(&json!({
        "promptFeedback": {
            "blockReason": "SAFETY",
            "blockReasonMessage": "The prompt is blocked due to safety"
        },
        "usageMetadata": {
            "promptTokenCount": 500,
            "cachedContentTokenCount": 100
        }
    }))
    .unwrap_err();
    assert_eq!(
        err,
        blyte_provider::ProviderError::NoAnswer {
            block_reason: Some("SAFETY".into()),
            block_reason_message: Some("The prompt is blocked due to safety".into()),
            usage: Usage {
                input: 400,
                cached: 100,
                output: 0,
                thinking: 0,
            },
        }
    );
    assert_eq!(
        err.to_string(),
        "Gemini returned no answer: prompt blocked (SAFETY): The prompt is blocked due to safety"
    );

    let empty_err = Gemini::parse(&json!({})).unwrap_err();
    assert_eq!(
        empty_err,
        blyte_provider::ProviderError::NoAnswer {
            block_reason: None,
            block_reason_message: None,
            usage: Usage::default(),
        }
    );
    assert_eq!(empty_err.to_string(), "Gemini returned no answer");

    let malformed = Gemini::parse(&json!({"candidates": ["not-an-object"]})).unwrap_err();
    assert!(
        matches!(malformed, blyte_provider::ProviderError::Parse(_)),
        "expected Parse, got {malformed:?}"
    );
}

#[test]
fn effort_suffix_becomes_thinking_level_and_leaves_the_model_id() {
    use blyte_provider::Provider;
    let g = gemini().with_model("gemini-3.8-flash-high");
    assert_eq!(g.model_id(), "gemini/gemini-3.8-flash");
    assert_eq!(g.thinking_level(), Some("high"));
    let b = g.body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert_eq!(
        b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "high"
    );
}

#[test]
fn a_bare_model_sends_no_generation_config() {
    let g = gemini().with_model("gemini-3.8-flash");
    assert_eq!(g.thinking_level(), None);
    let b = g.body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert!(b.get("generationConfig").is_none());
}

#[test]
fn unsupported_effort_suffixes_stay_in_the_model_id() {
    // -xhigh and -max have no Gemini equivalent. They must reach the API and fail loudly,
    // never run at a silently different effort.
    use blyte_provider::Provider;
    for m in ["gemini-3.8-flash-xhigh", "gemini-3.8-flash-max"] {
        let g = gemini().with_model(m);
        assert_eq!(g.model_id(), format!("gemini/{m}"));
        assert_eq!(g.thinking_level(), None);
    }
}

#[test]
fn thinking_level_and_max_output_tokens_coexist() {
    let g = gemini().with_model("gemini-3.8-flash-low");
    let b = g.body(&Request {
        messages: vec![Message::User("hi".into())],
        max_output_tokens: Some(1024),
        ..Default::default()
    });
    assert_eq!(b["generationConfig"]["maxOutputTokens"], 1024);
    assert_eq!(
        b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "low"
    );
}

#[test]
fn split_effort_handles_each_supported_level() {
    use blyte_provider::gemini::split_effort;
    assert_eq!(
        split_effort("gemini-3.8-flash-low"),
        ("gemini-3.8-flash".to_string(), Some("low".to_string()))
    );
    assert_eq!(
        split_effort("gemini-3.8-flash-medium"),
        ("gemini-3.8-flash".to_string(), Some("medium".to_string()))
    );
    assert_eq!(
        split_effort("gemini-3.8-flash-high"),
        ("gemini-3.8-flash".to_string(), Some("high".to_string()))
    );
    assert_eq!(
        split_effort("gemini-3.8-flash"),
        ("gemini-3.8-flash".to_string(), None)
    );
    assert_eq!(split_effort("-high"), ("-high".to_string(), None));
}

#[test]
fn a_parallel_batch_is_one_model_turn_and_one_reply() {
    // Gemini signs only the first call of a parallel batch, and rejects a model turn whose
    // first call is unsigned, so the batch must stay one content.
    let msgs = vec![
        Message::User("go".into()),
        Message::ToolCall {
            id: "a".into(),
            name: "read".into(),
            arguments: json!({"path": "a"}),
            signature: Some("sigA".into()),
        },
        Message::ToolCall {
            id: "b".into(),
            name: "read".into(),
            arguments: json!({"path": "b"}),
            signature: None,
        },
        Message::ToolResult {
            id: "a".into(),
            name: "read".into(),
            content: "A".into(),
            is_error: false,
        },
        Message::ToolResult {
            id: "b".into(),
            name: "read".into(),
            content: "B".into(),
            is_error: false,
        },
    ];
    let b = gemini().body(&Request {
        messages: msgs,
        ..Default::default()
    });
    let contents = b["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3, "{b}");
    assert_eq!(contents[1]["role"], "model");
    let calls = contents[1]["parts"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["thoughtSignature"], "sigA");
    assert!(calls[1].get("thoughtSignature").is_none());
    assert_eq!(contents[2]["role"], "user");
    let replies = contents[2]["parts"].as_array().unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["functionResponse"]["response"]["output"], "B");
}

#[test]
fn interleaved_calls_still_encode_one_per_turn() {
    let msgs = vec![
        Message::User("go".into()),
        Message::ToolCall {
            id: "a".into(),
            name: "read".into(),
            arguments: json!({"path": "a"}),
            signature: Some("s".into()),
        },
        Message::ToolResult {
            id: "a".into(),
            name: "read".into(),
            content: "A".into(),
            is_error: false,
        },
        Message::ToolCall {
            id: "b".into(),
            name: "read".into(),
            arguments: json!({"path": "b"}),
            signature: Some("t".into()),
        },
        Message::ToolResult {
            id: "b".into(),
            name: "read".into(),
            content: "B".into(),
            is_error: false,
        },
    ];
    let b = gemini().body(&Request {
        messages: msgs,
        ..Default::default()
    });
    assert_eq!(b["contents"].as_array().unwrap().len(), 5, "{b}");
}

fn assert_no_consecutive_same_role(contents: &[serde_json::Value]) {
    for window in contents.windows(2) {
        assert_ne!(
            window[0]["role"], window[1]["role"],
            "consecutive contents must not share a role: {} followed by {}",
            window[0]["role"], window[1]["role"]
        );
    }
}

#[test]
fn reply_with_text_and_tool_call_is_one_model_turn() {
    let msgs = vec![
        Message::User("go".into()),
        Message::Assistant("I'll look".into()),
        Message::ToolCall {
            id: "call_0".into(),
            name: "read_file".into(),
            arguments: json!({"path": "a.rs"}),
            signature: Some("sig".into()),
        },
        Message::ToolResult {
            id: "call_0".into(),
            name: "read_file".into(),
            content: "fn main(){}".into(),
            is_error: false,
        },
    ];
    let b = gemini().body(&Request {
        messages: msgs,
        tools: vec![tool()],
        ..Default::default()
    });
    let contents = b["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3, "{b}");
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[2]["role"], "user");

    let parts = contents[1]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], json!({"text": "I'll look"}));
    assert_eq!(
        parts[1],
        json!({
            "functionCall": {
                "name": "read_file",
                "args": {"path": "a.rs"}
            },
            "thoughtSignature": "sig"
        })
    );

    assert_no_consecutive_same_role(contents);
}

#[test]
fn reply_with_text_and_multiple_tool_calls_joins_into_single_model_turn() {
    let msgs = vec![
        Message::User("go".into()),
        Message::Assistant("a".into()),
        Message::ToolCall {
            id: "1".into(),
            name: "c1".into(),
            arguments: json!({"path": "a"}),
            signature: Some("sig1".into()),
        },
        Message::ToolCall {
            id: "2".into(),
            name: "c2".into(),
            arguments: json!({"path": "b"}),
            signature: None,
        },
        Message::ToolResult {
            id: "1".into(),
            name: "c1".into(),
            content: "res1".into(),
            is_error: false,
        },
        Message::ToolResult {
            id: "2".into(),
            name: "c2".into(),
            content: "res2".into(),
            is_error: false,
        },
    ];
    let b = gemini().body(&Request {
        messages: msgs,
        ..Default::default()
    });
    let contents = b["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3, "{b}");
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[2]["role"], "user");

    let model_parts = contents[1]["parts"].as_array().unwrap();
    assert_eq!(model_parts.len(), 3);
    assert_eq!(model_parts[0], json!({"text": "a"}));
    assert_eq!(
        model_parts[1],
        json!({
            "functionCall": {"name": "c1", "args": {"path": "a"}},
            "thoughtSignature": "sig1"
        })
    );
    assert_eq!(
        model_parts[2],
        json!({"functionCall": {"name": "c2", "args": {"path": "b"}}})
    );

    let user_parts = contents[2]["parts"].as_array().unwrap();
    assert_eq!(user_parts.len(), 2);
    assert_eq!(
        user_parts[0],
        json!({
            "functionResponse": {
                "name": "c1",
                "response": {"output": "res1", "error": false}
            }
        })
    );
    assert_eq!(
        user_parts[1],
        json!({
            "functionResponse": {
                "name": "c2",
                "response": {"output": "res2", "error": false}
            }
        })
    );

    assert_no_consecutive_same_role(contents);
}

#[test]
fn assistant_text_with_no_tool_calls_remains_unchanged() {
    let msgs = vec![
        Message::User("q".into()),
        Message::Assistant("answer".into()),
        Message::User("next".into()),
    ];
    let b = gemini().body(&Request {
        messages: msgs,
        ..Default::default()
    });
    let contents = b["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3, "{b}");
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[2]["role"], "user");

    assert_eq!(contents[0]["parts"][0], json!({"text": "q"}));
    assert_eq!(contents[1]["parts"][0], json!({"text": "answer"}));
    assert_eq!(contents[2]["parts"][0], json!({"text": "next"}));

    assert_no_consecutive_same_role(contents);
}
