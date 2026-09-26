# OpenAI-Compatible Provider — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API (tests use recorded JSON only); commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** blyte can drive any model behind an OpenAI-compatible Chat Completions API. `openai/<model>` uses `OPENAI_API_KEY` and `OPENAI_BASE_URL` (default `https://api.openai.com/v1`; a local server such as vLLM or LM Studio needs only the base URL). `openrouter/<model>` uses `OPENROUTER_API_KEY` against `https://openrouter.ai/api/v1`, which reaches Muse Spark, Claude, free models and more through one key.

**Why:** blyte speaks Gemini, Anthropic and Ollama. One adapter for the OpenAI wire format unlocks most other providers (OpenAI, OpenRouter, GitHub Models, Groq, local inference servers), so blyte can be measured across model families instead of on Gemini alone.

**Tech Stack:** Rust 2021, `ureq` (the crate's existing blocking HTTP client; no new dependencies).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified, with one exception: `test_discover_all_models_enumerates_providers` in `crates/blyte-provider/tests/test_registry.rs` expects exactly three providers. Update it to the five below (and have its env-clearing helper also remove `OPENAI_API_KEY`, `OPENAI_BASE_URL` and `OPENROUTER_API_KEY`).
- Follow the existing adapters (`crates/blyte-provider/src/ollama.rs` is the closest) for structure, doc-comment style, error mapping (`ProviderError::Api` for HTTP status errors, `Transport` for network errors, `Parse` for bad payloads) and the shared `crate::http_client()`.
- **Security:** the new credentials must never reach the model's shell. Add `OPENAI_API_KEY`, `OPENAI_BASE_URL` and `OPENROUTER_API_KEY` to `HARNESS_ENV` in `crates/blyte-agent/src/shell.rs`.
- No pricing entries: cost stays `None` for these models.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: The OpenAI-compatible adapter

**What to build** (the acceptance tests below check this contract):

`crates/blyte-provider/src/openai.rs`, exported as `pub mod openai` from `lib.rs`, with `pub struct OpenAi` implementing `Provider`:
- `OpenAi::openai_from_env() -> Result<OpenAi, ProviderError>`: key from `OPENAI_API_KEY`, base from `OPENAI_BASE_URL` (default `https://api.openai.com/v1`, trailing `/` trimmed). Fails with `MissingKey("OPENAI_API_KEY")` only when neither is set. Default model `gpt-5.5`, or `BLYTE_MODEL` with an `openai/` prefix stripped.
- `OpenAi::openrouter_from_env() -> Result<OpenAi, ProviderError>`: key from `OPENROUTER_API_KEY` (required: `MissingKey("OPENROUTER_API_KEY")`), base `https://openrouter.ai/api/v1`. Default model `openai/gpt-5.5`, or `BLYTE_MODEL` with an `openrouter/` prefix stripped.
- Builders `with_model`, `with_base_url`, `with_api_key`. `model_id()` is `openai/<model>` or `openrouter/<model>`.
- `body(&Request) -> serde_json::Value`: `model`; `messages` with the system prompt first (when non-empty) as `role: system`; `User` → `user`; `Assistant` → `assistant`; **consecutive `ToolCall` messages merged into one** `assistant` message whose `tool_calls` hold them all, each `{"id", "type": "function", "function": {"name", "arguments": <arguments serialised to a JSON string>}}` and `content` null; each `ToolResult` → `{"role": "tool", "tool_call_id", "content"}`; `tools` as `{"type": "function", "function": {name, description, parameters}}` only when non-empty; `max_completion_tokens` only when `max_output_tokens` is set.
- `OpenAi::parse(&Value) -> Result<Response, ProviderError>`: an `error` object fails with `Parse`; text from `choices[0].message.content` (null → empty); tool calls from `choices[0].message.tool_calls` (arguments are a JSON string: parse it, and use `{}` if it is not valid JSON; ids kept); `finish_reason` from `choices[0].finish_reason`; usage from `usage`: `cached` = `prompt_tokens_details.cached_tokens` (0 if absent), `input` = `prompt_tokens - cached`, `thinking` = `completion_tokens_details.reasoning_tokens` (0 if absent), `output` = `completion_tokens - thinking`.
- `complete` POSTs `body()` to `{base}/chat/completions` with `Authorization: Bearer <key>` when a key is set. `list_models` GETs `{base}/models` and returns each `data[].id` as a `ModelInfo`.

`crates/blyte-provider/src/registry.rs`:
- `ProviderKind::OpenAi` and `ProviderKind::OpenRouter`.
- `openai/` and `openrouter/` prefixes are detected **before** every other rule, so `openrouter/deepseek/deepseek-r1:free` (which contains `:`) is OpenRouter, not Ollama. The prefix is stripped before `with_model`.
- The ambient fallback (no model given) keeps its order and then tries `OPENAI_API_KEY`, then `OPENROUTER_API_KEY`, before the final Gemini default.
- `discover_all_models` also lists `"OpenAI"` and `"OpenRouter"`, after the existing three.

- [ ] **Step 1: Write the failing tests.**

Create `crates/blyte-provider/tests/test_openai_wire.rs`:

```rust
//! The OpenAI-compatible adapter: wire shape, response parsing and routing. Recorded JSON only.

use std::sync::Mutex;

use blyte_provider::openai::OpenAi;
use blyte_provider::registry::{detect_provider_kind, ProviderKind};
use blyte_provider::{resolve_provider, Message, ProviderError, Request, ToolSpec};
use serde_json::json;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn clear_env() {
    for k in ["GEMINI_API_KEY", "GOOGLE_API_KEY", "ANTHROPIC_API_KEY", "OLLAMA_API_KEY",
              "OLLAMA_HOST", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENROUTER_API_KEY",
              "BLYTE_MODEL"] {
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
            Message::ToolCall { id: "a".into(), name: "read".into(), arguments: json!({"path": "x"}), signature: None },
            Message::ToolCall { id: "b".into(), name: "bash".into(), arguments: json!({"command": "ls"}), signature: None },
            Message::ToolResult { id: "a".into(), name: "read".into(), content: "X".into(), is_error: false },
            Message::ToolResult { id: "b".into(), name: "bash".into(), content: "Y".into(), is_error: false },
        ],
        tools: vec![ToolSpec { name: "read".into(), description: "Read a file".into(),
                               parameters: json!({"type": "object"}) }],
        max_output_tokens: None,
    };
    let b = provider().body(&req);
    assert_eq!(b["model"], "gpt-5.5");
    let m = b["messages"].as_array().unwrap();
    assert_eq!(m.len(), 5, "system, user, ONE assistant with both calls, two tool results: {m:?}");
    assert_eq!(m[0]["role"], "system");
    assert_eq!(m[2]["role"], "assistant");
    let calls = m[2]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["function"]["name"], "read");
    assert!(calls[0]["function"]["arguments"].is_string(), "arguments go over the wire as a JSON string");
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
    })).unwrap();
    assert_eq!(r.text, "done");
    assert_eq!(r.finish_reason.as_deref(), Some("stop"));
    assert_eq!((r.usage.input, r.usage.cached, r.usage.output, r.usage.thinking), (400, 600, 200, 100));
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
    assert!(matches!(OpenAi::openai_from_env(), Err(ProviderError::MissingKey("OPENAI_API_KEY"))));
    std::env::set_var("OPENAI_BASE_URL", "http://localhost:8000/v1/");
    assert!(OpenAi::openai_from_env().is_ok());
    clear_env();
    assert!(matches!(OpenAi::openrouter_from_env(), Err(ProviderError::MissingKey("OPENROUTER_API_KEY"))));
}

#[test]
fn prefixes_route_before_the_ollama_colon_rule() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_env();
    assert_eq!(detect_provider_kind(Some("openai/gpt-5.5")), ProviderKind::OpenAi);
    assert_eq!(detect_provider_kind(Some("openrouter/deepseek/deepseek-r1:free")), ProviderKind::OpenRouter);
    assert_eq!(detect_provider_kind(Some("qwen3:14b")), ProviderKind::Ollama);

    std::env::set_var("OPENROUTER_API_KEY", "test-key-not-real");
    let p = resolve_provider(Some("openrouter/meta/muse-spark-1.3")).unwrap();
    assert_eq!(p.model_id(), "openrouter/meta/muse-spark-1.3");
    std::env::set_var("OPENAI_API_KEY", "test-key-not-real");
    let p = resolve_provider(Some("openai/gpt-5.5")).unwrap();
    assert_eq!(p.model_id(), "openai/gpt-5.5");
    clear_env();
}
```

Create `crates/blyte-agent/tests/test_shell_env_openai.rs`:

```rust
//! The new provider credentials never reach the model's shell.

use blyte_agent::shell::HARNESS_ENV;

#[test]
fn openai_and_openrouter_credentials_are_scrubbed() {
    for name in ["OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENROUTER_API_KEY"] {
        assert!(HARNESS_ENV.contains(&name), "{name} must be in HARNESS_ENV");
    }
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-provider --test test_openai_wire` and `cargo test -p blyte-agent --test test_shell_env_openai`. Expected: compile errors / failures.

- [ ] **Step 3: Implement** the adapter, the routing, the discovery entries, the `HARNESS_ENV` additions and the one allowed `test_registry.rs` update.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-provider` and `cargo test -p blyte-agent --test test_shell_env_openai --test test_shell_env`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(provider): OpenAI-compatible provider (openai/, openrouter/), credentials scrubbed from the shell"
```
