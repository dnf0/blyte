# Anthropic Prompt Caching — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API (tests use recorded JSON only); commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** blyte's Anthropic requests mark two cache breakpoints, so a run re-reads its prompt from
Anthropic's cache instead of paying full price for it every turn: `cache_control: {"type":
"ephemeral"}` on the **last tool** definition and on the **last content block of the last message**.
Usage counts cache writes: `input` = `input_tokens + cache_creation_input_tokens`.

**Why:** Anthropic caches only what a request marks, unlike Gemini's implicit cache. The adapter
marks nothing, so every Claude turn would bill the whole growing prompt at the full input price
(blyte's Gemini runs read 80–90% of their input from cache). The folded context only appends between
folds, so a breakpoint on the newest block caches everything before it and the next turn reads it
back. Cache writes (`cache_creation_input_tokens`) are also dropped from the usage today, which
undercounts tokens.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified (the system prompt stays a top-level string; the extra
  `cache_control` key does not change any field the existing tests check).
- Two breakpoints at most (Anthropic allows four). Nothing else in the request changes.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Mark cache breakpoints and count cache writes

**Files:** `crates/blyte-provider/src/anthropic.rs`; new test `crates/blyte-provider/tests/test_anthropic_cache.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-provider/tests/test_anthropic_cache.rs`:

```rust
//! Anthropic caches only what a request marks. The adapter marks the last tool and the newest
//! message block, so each turn reads the previous turn's prefix from cache; cache writes count.

use blyte_provider::anthropic::Anthropic;
use blyte_provider::{Message, Request, ToolSpec};
use serde_json::json;

fn anthropic() -> Anthropic {
    Anthropic::new("test-key-not-real").with_model("claude-sonnet-5")
}

fn tool(name: &str) -> ToolSpec {
    ToolSpec { name: name.into(), description: "d".into(), parameters: json!({"type": "object"}) }
}

fn req(messages: Vec<Message>) -> Request {
    Request { system: "sys".into(), messages, tools: vec![tool("read"), tool("bash")], max_output_tokens: None }
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
        Message::ToolCall { id: "t1".into(), name: "read".into(), arguments: json!({"path": "a"}), signature: None },
        Message::ToolResult { id: "t1".into(), name: "read".into(), content: "A".into(), is_error: false },
    ]));
    let msgs = b["messages"].as_array().unwrap();
    let last = msgs.last().unwrap()["content"].as_array().unwrap();
    assert_eq!(last.last().unwrap()["cache_control"]["type"], EPHEMERAL);
    // Exactly two breakpoints in the whole body: the last tool and the newest block.
    assert_eq!(b.to_string().matches("\"cache_control\"").count(), 2, "{b}");
}

#[test]
fn an_empty_conversation_still_marks_the_tools() {
    let b = anthropic().body(&Request { system: "sys".into(), messages: vec![], tools: vec![tool("read")], max_output_tokens: None });
    assert_eq!(b["tools"][0]["cache_control"]["type"], EPHEMERAL);
}

#[test]
fn cache_writes_count_as_input() {
    let r = Anthropic::parse(&json!({
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 50, "cache_creation_input_tokens": 3000,
                  "cache_read_input_tokens": 12000, "output_tokens": 20}
    })).unwrap();
    assert_eq!(r.usage.input, 3050, "fresh input includes the tokens written to the cache");
    assert_eq!(r.usage.cached, 12000);
    assert_eq!(r.usage.output, 20);
}
```

  If `Anthropic::new(key)` does not exist, add it: a constructor from an explicit key with the
  default model and API root, for tests and embedding (like `from_env` without the environment).

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-provider --test test_anthropic_cache`. Expected: failures (no `cache_control`; cache writes not counted).

- [ ] **Step 3: Implement.** In `crates/blyte-provider/src/anthropic.rs`:
  1. In `body()`, after the messages are built, add `"cache_control": {"type": "ephemeral"}` to the
     last content block of the last message (when there is one), and to the last entry of `tools`
     (when there are tools).
  2. In `parse()`, set `input` to `input_tokens + cache_creation_input_tokens` (each 0 if absent).
  3. Update the module doc comment: breakpoints on the last tool and the newest block; cache writes
     count as input.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-provider --test test_anthropic_cache --test test_anthropic_wire`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(provider): Anthropic requests mark cache breakpoints; cache writes count as input"
```
