# ChatGPT-Login Provider — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API (tests use recorded data only); commit locally only. Never read, print or modify the real `~/.codex/auth.json`: tests use a temporary `CODEX_HOME`. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `chatgpt/<model>` (for example `chatgpt/gpt-5.5`) runs blyte on OpenAI models through the
user's ChatGPT subscription, by reusing an existing Codex CLI login. It never runs its own login and
never refreshes tokens.

**Why:** OpenAI publicly allows third-party harnesses to use a ChatGPT login (as Pi and OpenCode do).
The user is signed in to Codex CLI, so blyte could use Codex-class models at no extra cost. Reuse only,
no refresh: Codex refresh tokens rotate and work once, so a harness that refreshes and writes the file
back imperfectly breaks the user's Codex CLI login. When the token is near expiry, blyte stops with a
clear message and the user runs any `codex` command, which refreshes it.

**Protocol** (from the open-source Codex CLI, `codex-rs`, and the Pi and OpenCode clients):
- **Credentials:** `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`):
  `{"tokens": {"access_token": "<JWT>", "account_id": "<id>"|null, ...}, "last_refresh": "..."}`.
  If `account_id` is null or missing, read it from the access-token JWT payload claim
  `"https://api.openai.com/auth"` → `"chatgpt_account_id"`. The token expiry is the JWT `exp` claim
  (seconds since the epoch).
- **Endpoint:** `POST https://chatgpt.com/backend-api/codex/responses`, with headers
  `Authorization: Bearer <access_token>`, `ChatGPT-Account-ID: <account_id>`,
  `Accept: text/event-stream`, `Content-Type: application/json`, `originator: blyte`,
  `User-Agent: blyte/<version>`.
- **Body (OpenAI Responses API):**
  `{"model", "instructions": <system prompt, or "You are a helpful assistant." when empty>,
  "input": [items], "tools": [...], "tool_choice": "auto", "parallel_tool_calls": true,
  "store": false, "stream": true}`.
  - Input items: user text → `{"type":"message","role":"user","content":[{"type":"input_text","text":…}]}`;
    assistant text → `{"type":"message","role":"assistant","content":[{"type":"output_text","text":…}]}`;
    a tool call → `{"type":"function_call","call_id","name","arguments":"<JSON string>"}`;
    a tool result → `{"type":"function_call_output","call_id","output":"<string>"}`.
  - Tools: `{"type":"function","name","description","parameters":<schema>,"strict":false}`.
- **Response:** Server-Sent Events. Lines `data: {json}`. Use:
  - `response.output_item.done`: its `item` is a complete `function_call` (a tool call: `call_id`,
    `name`, `arguments` JSON string) or a `message` (text: concatenate its `output_text` parts);
  - `response.completed`: `response.usage` =
    `{input_tokens, input_tokens_details:{cached_tokens}, output_tokens, output_tokens_details:{reasoning_tokens}}`;
  - `response.failed` or an `error` event: an error.

**Tech Stack:** Rust 2021, `ureq` and `serde_json` (already dependencies; no new ones).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified, except `test_discover_all_models_enumerates_providers` in
  `crates/blyte-provider/tests/test_registry.rs`, which counts providers: add `"ChatGPT"` to it (and
  have its env-clearing helper also remove `CODEX_HOME`).
- Follow `crates/blyte-provider/src/openai.rs` for structure, doc comments and error mapping.
- A small base64url decoder for the JWT payload is fine to write inline; do not add a dependency.
- No pricing entry: the subscription is not billed per token.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: The ChatGPT-login adapter

**What to build** (names are part of the contract):

`crates/blyte-provider/src/chatgpt.rs`, exported as `pub mod chatgpt`, with `pub struct ChatGpt`
implementing `Provider`:
- `ChatGpt::from_codex_home(home: &Path, now_unix: u64) -> Result<ChatGpt, ProviderError>`: reads
  `<home>/auth.json`. Errors (as `ProviderError::MissingKey("CODEX_HOME")` for a missing or unreadable
  file, otherwise `ProviderError::Parse(<message>)`): no `tokens.access_token`; no account id (field or
  JWT claim); the token expiring within 300 seconds of `now_unix`, with a message containing
  `run any codex command to refresh your ChatGPT login`.
- `ChatGpt::from_env()`: `from_codex_home($CODEX_HOME or ~/.codex, now)`. Default model `gpt-5.5`, or
  `BLYTE_MODEL` with a `chatgpt/` prefix stripped.
- `with_model`, `model_id()` = `chatgpt/<model>`, `account_id() -> &str`.
- `body(&Request) -> Value` and `ChatGpt::parse_sse(text: &str) -> Result<Response, ProviderError>`
  per the protocol above; `complete` POSTs `body()` with the headers above, reads the whole SSE body
  and returns `parse_sse`. Usage: `cached` = `cached_tokens`, `input` = `input_tokens - cached`,
  `thinking` = `reasoning_tokens`, `output` = `output_tokens - thinking` (saturating).

`crates/blyte-provider/src/registry.rs`: `ProviderKind::ChatGpt`; the `chatgpt/` prefix is detected
before every other rule and stripped; `discover_all_models` lists `"ChatGPT"` last (its model list
can be the fixed `["gpt-5.5"]`; no network call).

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-provider/tests/test_chatgpt.rs`:

```rust
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
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..(c.len() + 1) {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn jwt(exp: u64, account: &str) -> String {
    let payload = json!({"exp": exp, "https://api.openai.com/auth": {"chatgpt_account_id": account}});
    format!("{}.{}.sig", b64url(r#"{"alg":"none"}"#), b64url(&payload.to_string()))
}

fn home(auth: serde_json::Value) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("auth.json"), auth.to_string()).unwrap();
    d
}

const NOW: u64 = 1_800_000_000;

#[test]
fn a_valid_login_is_reused_and_the_account_id_comes_from_the_token_when_absent() {
    let h = home(json!({"tokens": {"access_token": jwt(NOW + 3600, "acct-123"), "account_id": null}}));
    let p = ChatGpt::from_codex_home(h.path(), NOW).unwrap().with_model("gpt-5.5");
    assert_eq!(p.account_id(), "acct-123");
    assert_eq!(blyte_provider::Provider::model_id(&p), "chatgpt/gpt-5.5");
}

#[test]
fn a_token_about_to_expire_is_refused_with_how_to_refresh_it() {
    let h = home(json!({"tokens": {"access_token": jwt(NOW + 60, "a"), "account_id": "a"}}));
    match ChatGpt::from_codex_home(h.path(), NOW) {
        Err(ProviderError::Parse(m)) => assert!(m.contains("run any codex command to refresh"), "{m}"),
        Err(e) => panic!("wrong error: {e:?}"),
        Ok(_) => panic!("an expiring token must be refused"),
    }
}

#[test]
fn a_missing_login_is_a_missing_key() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(ChatGpt::from_codex_home(d.path(), NOW), Err(ProviderError::MissingKey(_))));
}

#[test]
fn the_body_is_a_stateless_streaming_responses_request() {
    let h = home(json!({"tokens": {"access_token": jwt(NOW + 3600, "a"), "account_id": "a"}}));
    let p = ChatGpt::from_codex_home(h.path(), NOW).unwrap().with_model("gpt-5.5");
    let b = p.body(&Request {
        system: String::new(),
        messages: vec![
            Message::User("fix it".into()),
            Message::ToolCall { id: "c1".into(), name: "read".into(), arguments: json!({"path": "a"}), signature: None },
            Message::ToolResult { id: "c1".into(), name: "read".into(), content: "A".into(), is_error: false },
        ],
        tools: vec![ToolSpec { name: "read".into(), description: "Read".into(), parameters: json!({"type": "object"}) }],
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
    assert_eq!((r.usage.input, r.usage.cached, r.usage.output, r.usage.thinking), (400, 600, 200, 100));
}

#[test]
fn a_failed_response_is_an_error() {
    let sse = r#"data: {"type":"response.failed","response":{"error":{"message":"quota"}}}"#;
    assert!(ChatGpt::parse_sse(sse).is_err());
}

#[test]
fn the_prefix_routes_before_the_ollama_colon_rule() {
    assert_eq!(detect_provider_kind(Some("chatgpt/gpt-5.5")), ProviderKind::ChatGpt);
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-provider --test test_chatgpt`. Expected: compile errors.

- [ ] **Step 3: Implement** the adapter, the routing, the discovery entry and the one allowed `test_registry.rs` update.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-provider`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(provider): ChatGPT-login provider (chatgpt/), reusing a Codex CLI login without refreshing it"
```
