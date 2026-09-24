# Parallel Tool Calls Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Let the model batch independent tool calls into one turn, safely, and invite it to.

**Why (superstate-eval FINDINGS §13.36 and the 2026-09-24 log analysis):** In all four passing `build-cython-ext` runs (`conversational` and `folded`), the model issued **exactly one tool call per turn**: 178–234 turns, each a full model call of about 3 seconds. Gemini supports parallel function calls and our loop executes several per turn, but two things stand in the way:
1. **Batched calls would break.** The loop records each call directly followed by its result, and the Gemini adapter sends each call as its own model turn. For a parallel batch Gemini puts the thought signature on the **first** call only, and rejects a model turn whose first call lacks one (see the `Message::ToolCall` docs). `StateOnly`, `StateWindow` and `Folded` also assume a call sits directly before its result.
2. **Nothing invites batching.** The system prompt never mentions it.

**Design:**
- **Turn blocks.** The loop records a turn's calls together, then its results together, in call order: `[C1, C2, R1, R2]`.
- **Gemini adapter.** Consecutive `ToolCall`s become one model content with several `functionCall` parts, and consecutive `ToolResult`s become one user content with several `functionResponse` parts. Interleaved histories encode exactly as before.
- **Anthropic adapter.** Unchanged: it already merges consecutive same-role messages.
- **Context strategies work in whole turn blocks.**
  - `StateOnly`'s latest observation is the last block.
  - `StateWindow` carries whole blocks.
  - `Folded` cuts only at block boundaries, and pairs digest lines with their results by id.
- **Prompt.** A short "Working efficiently" section invites batching independent calls (Task 2).

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/parallel-tool-calls`. Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. Task 2 requires Task 1.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass unmodified. Commit with `--no-gpg-sign` and the exact messages below.

---

### Task 1: Turn blocks through the loop, the Gemini adapter and the context strategies

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`, `crates/superstate-agent/src/context.rs`, `crates/superstate-provider/src/gemini.rs`
- Test: `crates/superstate-agent/tests/test_parallel_calls.rs` (create), `crates/superstate-provider/tests/test_gemini_wire.rs` (append)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_parallel_calls.rs`:

```rust
//! A turn with several tool calls is one block: its calls, then its results, in call order.
//! Every provider encoding and context strategy keeps a block whole.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{
    ContextStrategy, Conversational, Folded, StateOnly, StateWindow,
};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Runner};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

struct Scripted {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
        Ok(if s.is_empty() { Response::default() } else { s.remove(0) })
    }
    fn model_id(&self) -> String {
        "scripted".into()
    }
}

fn tc(id: &str, path: &str, sig: Option<&str>) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: json!({"path": path}),
        signature: sig.map(String::from),
    }
}

fn call(n: usize, sig: bool) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: "bash".into(),
        arguments: json!({"command": format!("grep -rn thing{n} src")}),
        signature: sig.then(|| format!("sig{n}")),
    }
}

fn result(n: usize, body: &str) -> Message {
    Message::ToolResult {
        id: format!("c{n}"),
        name: "bash".into(),
        content: body.into(),
        is_error: false,
    }
}

/// Opening message, then `turns` blocks of two parallel calls each: [Ca, Cb, Ra, Rb].
fn batched(turns: usize, body_chars: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for t in 0..turns {
        let (a, b) = (2 * t + 1, 2 * t + 2);
        h.push(call(a, true));
        h.push(call(b, false));
        h.push(result(a, &"x".repeat(body_chars)));
        h.push(result(b, &"y".repeat(body_chars)));
    }
    h
}

fn kinds(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .map(|m| match m {
            Message::User(_) => "U".into(),
            Message::Assistant(_) => "A".into(),
            Message::ToolCall { id, .. } => format!("C:{id}"),
            Message::ToolResult { id, .. } => format!("R:{id}"),
        })
        .collect()
}

/// Every result's call is present, and every call's result is present.
fn blocks_whole(msgs: &[Message]) -> bool {
    let calls: Vec<&String> = msgs
        .iter()
        .filter_map(|m| match m {
            Message::ToolCall { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    let results: Vec<&String> = msgs
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    calls.iter().all(|c| results.contains(c)) && results.iter().all(|r| calls.contains(r))
}

#[test]
fn the_loop_records_a_turns_calls_then_its_results() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
    std::fs::write(d.path().join("b.txt"), "b\n").unwrap();
    let p = Scripted {
        script: Mutex::new(vec![
            Response {
                tool_calls: vec![tc("r1", "a.txt", Some("s1")), tc("r2", "b.txt", None)],
                ..Default::default()
            },
            Response {
                text: "done".into(),
                ..Default::default()
            },
        ]),
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };
    let mut st = State::new("read both", "true");
    let (outcome, report) = runner.run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.tool_calls, 2);
    let second = &p.seen.lock().unwrap()[1];
    assert_eq!(kinds(&second.messages), vec!["U", "C:r1", "C:r2", "R:r1", "R:r2"]);
}

#[test]
fn state_only_carries_the_whole_last_block() {
    let out = StateOnly.build(&State::new("fix it", ""), &batched(3, 10));
    assert_eq!(kinds(&out[1..]), vec!["C:c5", "C:c6", "R:c5", "R:c6"]);
}

#[test]
fn state_window_carries_whole_blocks() {
    let w = StateWindow {
        results: 1,
        lines_per_result: 60,
    };
    let out = w.build(&State::new("fix it", ""), &batched(3, 10));
    assert_eq!(kinds(&out[1..]), vec!["C:c5", "C:c6", "R:c5", "R:c6"]);
    assert!(blocks_whole(&out));
}

#[test]
fn folded_cuts_only_between_blocks() {
    let st = State::new("fix it", "");
    let out = Folded { budget_tokens: 2_000 }.build(&st, &batched(80, 200));
    let digests: Vec<&String> = out
        .iter()
        .filter_map(|m| match m {
            Message::User(t) if t.starts_with("## Earlier turns, folded") => Some(t),
            _ => None,
        })
        .collect();
    assert!(!digests.is_empty(), "expected a fold");
    let tail = &out[1 + digests.len()..];
    assert!(matches!(tail[0], Message::ToolCall { .. }), "{:?}", kinds(tail));
    assert!(blocks_whole(tail), "{:?}", kinds(tail));
    // Both calls of a folded block are summarised, each with its own result.
    assert!(digests[0].contains("`grep -rn thing1 src`"), "{}", digests[0]);
    assert!(digests[0].contains("`grep -rn thing2 src`"), "{}", digests[0]);
    assert!(!digests[0].contains("no result"), "{}", digests[0]);
}
```

Append to `crates/superstate-provider/tests/test_gemini_wire.rs`:

```rust
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
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_parallel_calls` and `cargo test -p superstate-provider --test test_gemini_wire`. Expected failures:
  - `the_loop_records_a_turns_calls_then_its_results`: order is `C:r1, R:r1, C:r2, R:r2`.
  - `state_only_carries_the_whole_last_block`: only `C?`/`R:c6`.
  - `state_window_carries_whole_blocks`: a split block.
  - `folded_cuts_only_between_blocks`: a tail opening mid-block, or "no result" in the digest.
  - `a_parallel_batch_is_one_model_turn_and_one_reply`: 5 contents.
  - `interleaved_calls_still_encode_one_per_turn` should already pass.

- [ ] **Step 3: The loop records turn blocks.** In `crates/superstate-agent/src/run.rs`, in `Runner::run_until`, replace

```rust
                for call in &resp.tool_calls {
                    report.tool_calls += 1;
                    history.push(Message::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        signature: call.signature.clone(),
                    });
```

with

```rust
                // A turn's calls are recorded together, then its results together, in call
                // order. Providers encode one model turn carrying several calls and one reply
                // carrying their results; interleaving them split a parallel batch into turns
                // Gemini rejects (it signs only the first call of a batch).
                for call in &resp.tool_calls {
                    history.push(Message::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        signature: call.signature.clone(),
                    });
                }
                for call in &resp.tool_calls {
                    report.tool_calls += 1;
```

Leave the rest of the loop body, including the `history.push(Message::ToolResult { … })` at its end, unchanged.

- [ ] **Step 4: The Gemini adapter groups a block.** In `crates/superstate-provider/src/gemini.rs`:
  1. Directly above `pub struct Gemini {` (above its doc comment if any), add:

```rust
/// Append a part to the last content. Used to keep a parallel batch in one content.
fn push_part(contents: &mut [Value], part: Value) {
    if let Some(parts) = contents.last_mut().and_then(|c| c["parts"].as_array_mut()) {
        parts.push(part);
    }
}
```

  2. In `Gemini::body`, replace the whole `for m in &req.messages { … }` loop with:

```rust
        let mut prev: Option<&Message> = None;
        for m in &req.messages {
            match m {
                Message::User(t) => contents.push(json!({
                    "role": "user", "parts": [{"text": t}]
                })),
                Message::Assistant(t) => contents.push(json!({
                    "role": "model", "parts": [{"text": t}]
                })),
                Message::ToolCall {
                    name,
                    arguments,
                    signature,
                    ..
                } => {
                    let mut part = json!({"functionCall": {"name": name, "args": arguments}});
                    // Echoed verbatim. Gemini 3.x binds the model's reasoning to the call and
                    // rejects the turn outright if it is absent.
                    if let Some(sig) = signature {
                        part["thoughtSignature"] = json!(sig);
                    }
                    // Calls from one turn are one model content: Gemini signs only the first call
                    // of a parallel batch, so a batch split into separate turns would be rejected.
                    if matches!(prev, Some(Message::ToolCall { .. })) {
                        push_part(&mut contents, part);
                    } else {
                        contents.push(json!({"role": "model", "parts": [part]}));
                    }
                }
                // Gemini carries no tool-call id: a functionResponse is matched to its call by
                // NAME and position. Ids are kept in our own Message type anyway, because the
                // loop needs them and other providers require them.
                Message::ToolResult {
                    name,
                    content,
                    is_error,
                    ..
                } => {
                    let part = json!({"functionResponse": {
                        "name": name,
                        "response": {"output": content, "error": is_error}
                    }});
                    if matches!(prev, Some(Message::ToolResult { .. })) {
                        push_part(&mut contents, part);
                    } else {
                        contents.push(json!({"role": "user", "parts": [part]}));
                    }
                }
            }
            prev = Some(m);
        }
```

- [ ] **Step 5: `StateOnly` carries the last whole block.** In `crates/superstate-agent/src/context.rs`, in `StateOnly::build`, replace

```rust
        if let Some(pos) = history
            .iter()
            .rposition(|m| matches!(m, Message::ToolResult { .. }))
        {
            if pos > 0 {
                if let Message::ToolCall { .. } = &history[pos - 1] {
                    out.push(history[pos - 1].clone());
                }
            }
            out.push(history[pos].clone());
        } else if let Some(last) = history.last() {
```

with

```rust
        if let Some(pos) = history
            .iter()
            .rposition(|m| matches!(m, Message::ToolResult { .. }))
        {
            // The whole last turn block: its calls, then its results.
            let start = block_start(history, pos);
            out.extend_from_slice(&history[start..=pos]);
        } else if let Some(last) = history.last() {
```

and add this function directly above `impl ContextStrategy for StateOnly {`:

```rust
/// Start of the turn block ending at `pos` (a `ToolResult`): the run of results ending there,
/// preceded by the run of calls that produced them.
fn block_start(history: &[Message], pos: usize) -> usize {
    let mut start = pos;
    while start > 0 && matches!(history[start - 1], Message::ToolResult { .. }) {
        start -= 1;
    }
    while start > 0 && matches!(history[start - 1], Message::ToolCall { .. }) {
        start -= 1;
    }
    start
}
```

- [ ] **Step 6: `StateWindow` carries whole blocks.** In `impl ContextStrategy for StateWindow`, replace everything from `// Walk back collecting call/result pairs.` through the end of the `for (call, result) in pairs.into_iter().rev() { … }` loop with:

```rust
        // Walk back collecting whole turn blocks (a turn's calls, then its results) until at
        // least `results` results are carried. A block is never split: a result without its
        // call, or a call without its result, is rejected by providers.
        let mut blocks: Vec<Vec<Message>> = Vec::new();
        let mut carried = 0;
        let mut end = history.len();
        while carried < self.results {
            let Some(pos) = history[..end]
                .iter()
                .rposition(|m| matches!(m, Message::ToolResult { .. }))
            else {
                break;
            };
            let start = block_start(history, pos);
            if !matches!(history[start], Message::ToolCall { .. }) {
                break;
            }
            let block: Vec<Message> = history[start..=pos]
                .iter()
                .map(|m| match m {
                    Message::ToolResult {
                        id,
                        name,
                        content,
                        is_error,
                    } => Message::ToolResult {
                        id: id.clone(),
                        name: name.clone(),
                        content: crate::tools::distil(content, self.lines_per_result),
                        is_error: *is_error,
                    },
                    other => other.clone(),
                })
                .collect();
            carried += block
                .iter()
                .filter(|m| matches!(m, Message::ToolResult { .. }))
                .count();
            blocks.push(block);
            end = start;
        }
        for block in blocks.into_iter().rev() {
            out.extend(block);
        }
```

Keep the trailing "Whatever came after the last result" block unchanged.

- [ ] **Step 7: `Folded` cuts between blocks and pairs by id.**
  1. In `Folded::build`, replace

```rust
            let Some(cut) = (lo..=i).find(|&j| !matches!(rest[j], Message::ToolResult { .. }))
            else {
                continue;
            };
```

     with

```rust
            let Some(cut) = (lo..=i).find(|&j| {
                !matches!(rest[j], Message::ToolResult { .. })
                    && !matches!(rest[j - 1], Message::ToolCall { .. })
            }) else {
                continue;
            };
```

     and update the comment above it to read: `// Fold roughly the older half, cutting only between turn blocks (never after a call or before a result), so a call is never separated from its result or from calls in the same batch. No safe cut yet: keep accumulating.`

  2. Replace the whole `fn digest(folded: &[Message], first: usize, last: usize) -> Message { … }` with:

```rust
/// One harness-written line per folded item, from the same summarisers as the action ledger.
/// A call is paired with its result by id, so a parallel batch summarises correctly.
fn digest(folded: &[Message], first: usize, last: usize) -> Message {
    let mut text = format!(
        "## Earlier turns, folded (messages {first}-{last}; summarised by the harness)\n"
    );
    let result_of = |id: &str| {
        folded.iter().find_map(|m| match m {
            Message::ToolResult {
                id: rid,
                content,
                is_error,
                ..
            } if rid == id => Some((content.as_str(), *is_error)),
            _ => None,
        })
    };
    for m in folded {
        match m {
            Message::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                let target = crate::run::extract_action_target(name, arguments);
                let summary = match result_of(id) {
                    Some((content, is_error)) => {
                        crate::run::extract_action_summary(name, content, is_error)
                    }
                    None => "no result".to_string(),
                };
                text.push_str(&format!("- `{name}` `{target}` -> {summary}\n"));
            }
            Message::Assistant(t) => text.push_str(&format!("- you said: {}\n", clip(t, 160))),
            Message::User(t) => text.push_str(&format!("- harness: {}\n", clip(t, 160))),
            Message::ToolResult { .. } => {}
        }
    }
    Message::User(text)
}
```

- [ ] **Step 8: Run the tests.** Run: `cargo test -p superstate-agent --test test_parallel_calls --test test_state_window --test test_folded` and `cargo test -p superstate-provider --test test_gemini_wire`. Expected: all pass (4, 7, 6, and the Gemini wire suite with its 2 new tests).

- [ ] **Step 9: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 10: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): parallel tool calls travel as whole turn blocks"
```

---

### Task 2: Invite batching in the system prompt

Requires Task 1 merged.

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Test: `crates/superstate-agent/tests/test_parallel_calls.rs` (append)

- [ ] **Step 1: Write the failing test.** Append to `crates/superstate-agent/tests/test_parallel_calls.rs`:

```rust
#[test]
fn the_system_prompt_invites_batching_independent_calls() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted {
        script: Mutex::new(vec![Response {
            text: "done".into(),
            ..Default::default()
        }]),
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 2,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };
    runner.run(&mut State::new("anything", "true"));
    let system = p.seen.lock().unwrap()[0].system.clone();
    assert!(system.contains("## Working efficiently"), "{system}");
    assert!(system.contains("several tool calls in one turn"), "{system}");
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p superstate-agent --test test_parallel_calls the_system_prompt_invites_batching_independent_calls`. Expected: FAIL (no such section).

- [ ] **Step 3: Add the section.** In `Runner::system_prompt` in `crates/superstate-agent/src/run.rs`, in the format string, directly after the line `             ## Environment\n{where_}\nAvailable: {bins}\n\n\`, insert:

```rust
             ## Working efficiently\n\
             Independent reads, searches and commands can be issued as several tool calls in one \
             turn: they run in order and you see every result together. Batch them whenever one \
             does not depend on another's result; every turn you save is a full model call.\n\n\
```

- [ ] **Step 4: Run the tests.** Run: `cargo test -p superstate-agent --test test_parallel_calls`. Expected: 5 passed.

- [ ] **Step 5: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [ ] **Step 6: Commit.**

```bash
git add crates/superstate-agent/
git commit --no-gpg-sign -m "feat(agent): system prompt invites batching independent tool calls"
```
