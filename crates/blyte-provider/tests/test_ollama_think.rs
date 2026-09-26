//! Thinking is off unless asked for: local Qwen3 spent 435 s thinking through a 4-turn task
//! (31.3 s against 1.8 s per answer). `"think": false` is safe on every model; `true` is
//! rejected by models without thinking, so it is an explicit `+think` opt-in.

use blyte_provider::ollama::Ollama;
use blyte_provider::{Message, Request};

fn req() -> Request {
    Request {
        system: String::new(),
        messages: vec![Message::User("hi".into())],
        tools: vec![],
        max_output_tokens: None,
    }
}

fn local() -> Ollama {
    Ollama::from_env_or_default().with_api_root("http://localhost:11434")
}

#[test]
fn thinking_is_off_by_default() {
    let b = local().with_model("qwen3:14b").body(&req());
    assert_eq!(b["think"], false);
    assert_eq!(b["model"], "qwen3:14b");
}

#[test]
fn a_think_suffix_turns_it_on_and_is_stripped_from_the_wire() {
    let b = local().with_model("qwen3:14b+think").body(&req());
    assert_eq!(b["think"], true);
    assert_eq!(b["model"], "qwen3:14b");
}

#[test]
fn the_suffix_works_with_the_provider_prefix() {
    let b = local().with_model("ollama/qwen3:14b+think").body(&req());
    assert_eq!(b["think"], true);
    assert_eq!(b["model"], "qwen3:14b");
}
