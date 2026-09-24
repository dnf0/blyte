# State-Window Context Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** A context strategy that keeps the last few tool results in view, so the worker stops re-running what it has already seen, while the prompt stays bounded in session length.

**Why (superstate-eval FINDINGS §13.33 and the 2026-09-23 harness canary):** `StateOnly` sends only the **latest** tool call and result (`context.rs`: "if it mattered, it belonged in state"). The worker rarely writes state, so every search result lived for one turn, and it was run again:
- **`scc`:** 6 files re-read 101 times, and no edits.
- **`build-cython-ext`:** the same `grep -rn "n\.float"` 14 times, and 295 bash calls of which only 124 were distinct. It found the fix, made 3 edits, and spent the last three quarters of the run re-checking. agy passed that task (11/11, $0.88); superstate didn't (7/11, $2.59).

A second, quieter consequence: when a turn makes several tool calls, `StateOnly` shows only the last result.

**Design:** A new `StateWindow` strategy, named `state-window`:
- It sends `StateOnly`'s prompt (task, state, working set, action ledger) plus the last `results` tool call/result pairs, oldest first, each result cut to `lines_per_result` lines with `tools::distil`.
- If the latest message is not a tool result (for example gate feedback), it is appended.
- Defaults: 8 results, 60 lines each. The prompt is bounded by `1 + 2*results + 1` messages whatever the session length.
- `StateOnly` is **unchanged**. It stays as the control, so the eval can compare `state-only`, `state-window` and `conversational` as harness settings.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/state-window-context`. The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` is known to be timing-flaky: if it alone fails, re-run once and report both results.
- Existing tests must pass unmodified.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `StateWindow` and `--context state-window`

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`, `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`
- Test: `crates/superstate-agent/tests/test_state_window.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_state_window.rs`:

```rust
//! `state-window`: state-only's bounded prompt plus the last few tool results.

use serde_json::json;
use superstate_agent::context::{ContextStrategy, StateOnly, StateWindow};
use superstate_agent::state::State;
use superstate_provider::Message;

fn call(n: usize) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: "bash".into(),
        arguments: json!({"command": format!("grep -rn thing{n}")}),
        signature: None,
    }
}

fn result(n: usize, content: &str) -> Message {
    Message::ToolResult {
        id: format!("c{n}"),
        name: "bash".into(),
        content: content.into(),
        is_error: false,
    }
}

/// `n` call/result pairs after the opening message, as the run loop records them.
fn history(n: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for i in 1..=n {
        h.push(call(i));
        h.push(result(i, &format!("result {i}")));
    }
    h
}

fn ids(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            Message::ToolCall { id, .. } => Some(format!("call:{id}")),
            Message::ToolResult { id, .. } => Some(format!("result:{id}")),
            _ => None,
        })
        .collect()
}

fn window(results: usize, lines: usize) -> StateWindow {
    StateWindow {
        results,
        lines_per_result: lines,
    }
}

#[test]
fn keeps_the_last_results_in_order_and_drops_older_ones() {
    let st = State::new("fix it", "");
    let out = window(3, 60).build(&st, &history(10));
    assert!(matches!(out[0], Message::User(_)), "the state prompt comes first");
    assert_eq!(
        ids(&out),
        vec![
            "call:c8", "result:c8", "call:c9", "result:c9", "call:c10", "result:c10"
        ]
    );
}

#[test]
fn long_results_are_cut_to_the_line_budget() {
    let st = State::new("fix it", "");
    let long: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    let h = vec![Message::User("Begin.".into()), call(1), result(1, &long)];
    let out = window(8, 20).build(&st, &h);
    let Message::ToolResult { content, .. } = out.last().unwrap() else {
        panic!("expected the result last");
    };
    assert!(content.lines().count() < 30, "{} lines", content.lines().count());
    assert!(content.contains("elided by superstate"));
}

#[test]
fn the_prompt_stays_bounded_however_long_the_session() {
    let st = State::new("fix it", "");
    let w = StateWindow::default();
    let out = w.build(&st, &history(500));
    assert!(out.len() <= 1 + 2 * w.results + 1, "{} messages", out.len());
    assert_eq!(w.results, 8);
    assert_eq!(w.lines_per_result, 60);
}

#[test]
fn gate_feedback_after_the_results_is_kept_last() {
    let st = State::new("fix it", "");
    let mut h = history(2);
    h.push(Message::User("Verification did not pass.\n\nexit 1".into()));
    let out = window(8, 60).build(&st, &h);
    match out.last().unwrap() {
        Message::User(t) => assert!(t.starts_with("Verification did not pass")),
        other => panic!("expected gate feedback last, got {other:?}"),
    }
    assert_eq!(ids(&out).len(), 4);
}

#[test]
fn every_result_of_a_multi_call_turn_is_visible() {
    let st = State::new("fix it", "");
    let h = vec![
        Message::User("Begin.".into()),
        call(1),
        result(1, "a"),
        call(2),
        result(2, "b"),
    ];
    let out = window(8, 60).build(&st, &h);
    assert_eq!(ids(&out), vec!["call:c1", "result:c1", "call:c2", "result:c2"]);
}

#[test]
fn the_first_turn_carries_the_opening_message() {
    let st = State::new("fix it", "");
    let out = window(8, 60).build(&st, &history(0));
    assert_eq!(out.len(), 2);
    assert!(matches!(&out[1], Message::User(t) if t.starts_with("Begin.")));
}

#[test]
fn state_only_is_unchanged_as_the_control() {
    let st = State::new("fix it", "");
    let out = StateOnly.build(&st, &history(10));
    assert_eq!(ids(&out), vec!["call:c10", "result:c10"]);
    assert_eq!(StateWindow::default().name(), "state-window");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_state_window`. Expected: compile error, because `StateWindow` does not exist.

- [ ] **Step 3: Add the strategy.** In `crates/superstate-agent/src/context.rs`, append at the end of the file:

```rust
/// `state-only` plus the last few tool results.
///
/// `StateOnly` sends only the latest tool call and result, on the theory that anything worth
/// keeping belongs in state. In practice the worker rarely writes state, so every search result
/// lived for one turn and was run again: the same grep 14 times on build-cython-ext, six files
/// re-read 101 times on scc (superstate-eval FINDINGS 13.33). This keeps recent results in view
/// and stays bounded in session length: at most `1 + 2 * results + 1` messages.
#[derive(Debug, Clone)]
pub struct StateWindow {
    /// How many of the most recent tool call/result pairs to carry.
    pub results: usize,
    /// Each carried result is cut to this many lines with `tools::distil`.
    pub lines_per_result: usize,
}

impl Default for StateWindow {
    fn default() -> Self {
        Self {
            results: 8,
            lines_per_result: 60,
        }
    }
}

impl ContextStrategy for StateWindow {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        // The state prompt, exactly as `state-only` builds it with nothing carried.
        let mut out = StateOnly.build(state, &[]);

        // Walk back collecting call/result pairs. The call is carried with its result because
        // providers reject a functionResponse with no matching functionCall.
        let mut pairs: Vec<(Message, Message)> = Vec::new();
        for i in (1..history.len()).rev() {
            if pairs.len() == self.results {
                break;
            }
            if let (
                Message::ToolCall { .. },
                Message::ToolResult {
                    id,
                    name,
                    content,
                    is_error,
                },
            ) = (&history[i - 1], &history[i])
            {
                pairs.push((
                    history[i - 1].clone(),
                    Message::ToolResult {
                        id: id.clone(),
                        name: name.clone(),
                        content: crate::tools::distil(content, self.lines_per_result),
                        is_error: *is_error,
                    },
                ));
            }
        }
        for (call, result) in pairs.into_iter().rev() {
            out.push(call);
            out.push(result);
        }

        // Whatever came after the last result (gate feedback, or the opening message on the
        // first turn) is the newest thing the worker must see.
        if let Some(last) = history.last() {
            if !matches!(last, Message::ToolResult { .. }) {
                out.push(last.clone());
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "state-window"
    }
}
```

- [ ] **Step 4: Expose it on the CLI.**
  1. In `crates/superstate-cli/src/cli.rs`, in the `context` argument, replace `value_parser = ["state-only", "conversational"]` with `value_parser = ["state-only", "state-window", "conversational"]`. Replace the doc comment line above the attribute (`/// Context strategy: 'state-only' (SKILL.state O(1) prompt) or 'conversational' (append-all chat)`) with `/// Context strategy: 'state-only' (O(1) prompt, latest result only), 'state-window' (O(1) prompt plus the last 8 results) or 'conversational' (append-all chat)`.
  2. In `crates/superstate-cli/src/task_runner.rs`, replace

```rust
    let state_only = superstate_agent::context::StateOnly;
    let conversational = superstate_agent::context::Conversational;
    let context_strategy: &dyn superstate_agent::context::ContextStrategy =
        if config.context_strategy == "state-only" {
            &state_only
        } else {
            &conversational
        };
```

     with

```rust
    let state_only = superstate_agent::context::StateOnly;
    let state_window = superstate_agent::context::StateWindow::default();
    let conversational = superstate_agent::context::Conversational;
    let context_strategy: &dyn superstate_agent::context::ContextStrategy =
        match config.context_strategy.as_str() {
            "state-only" => &state_only,
            "state-window" => &state_window,
            _ => &conversational,
        };
```

- [ ] **Step 5: Run the new tests.** Run: `cargo test -p superstate-agent --test test_state_window`. Expected: 7 passed.

- [ ] **Step 6: Check the CLI accepts it.** Run: `cargo run -q --bin superstate -- --help | grep -A1 -- "--context"`. Expected: the possible values list `state-only, state-window, conversational`.

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 8: Commit.**

```bash
git add crates/superstate-agent/ crates/superstate-cli/
git commit --no-gpg-sign -m "feat(agent): state-window context keeps the last tool results in view"
```
