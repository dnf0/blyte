# Folded Context Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** A context strategy that keeps the conversation **append-only**, as Gemini expects, but bounded: once the verbatim history passes a token budget, its oldest half is folded into a harness-written digest.

**Why:**
- **The bounded contexts lose the thread.** `state-only` and `state-window` rebuild the prompt every turn. In every eval run the worker lost track of its own work (superstate-eval FINDINGS §13.33–13.35). They also drop Gemini's thought signatures. Google's guidance: "You MUST always resend all `thought` blocks exactly as they were received … You should NOT remove or modify thought blocks from the history, as they contain the signatures required for the model to continue its reasoning."
- **A rebuilt prompt can't share a long prefix,** so it caches badly (at most 9%).
- **SKILL.state (arXiv:2608.26263v2) only makes discarding safe by requiring the model to write a state patch every turn.** It names this setting (schema discovered during execution, relevance recognised late) as the one where discarding fails.

**Design (`Folded`, name `folded`, default budget 48,000 approximate tokens):**
- **Output:** `[opening message] [digest 1] … [digest k] [verbatim tail]`.
- **Pure and prefix-stable.** Fold boundaries are computed greedily from the start of history. A boundary depends only on the messages before it, so turn N+1 reproduces turn N's folds exactly. Between folds the output is pure append, so caching and signatures are preserved. Each fold appends one digest, so the cache misses once and then warms up again.
- **Never splits a call from its result.** A cut is only ever placed before a message that is not a `ToolResult`. The tail never starts with an orphaned result, which providers reject.
- **Deterministic digests, no extra model call.** One line per folded item, using the ledger's existing `extract_action_target` / `extract_action_summary`: tool calls with their result summary, the model's own text (clipped), and harness messages such as gate feedback (first line).
- **Below the budget,** the output is identical to `Conversational`.
- Size is estimated as characters / 4. It's a budget, not a billing figure.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/folded-context`. The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` is known to be timing-flaky: if it alone fails, re-run once and report both results.
- Existing tests must pass unmodified. Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `Folded` and `--context folded`

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`, `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`
- Test: `crates/superstate-agent/tests/test_folded.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_folded.rs`:

```rust
//! `folded`: an append-only conversation, bounded by folding its oldest half into
//! harness-written digests once it passes a token budget.

use serde_json::json;
use superstate_agent::context::{ContextStrategy, Conversational, Folded};
use superstate_agent::state::State;
use superstate_provider::Message;

fn call(n: usize) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: "bash".into(),
        arguments: json!({"command": format!("grep -rn thing{n} src")}),
        signature: Some(format!("sig{n}")),
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

/// Opening message, then `n` turns of: model text, a call, its result.
fn history(n: usize, result_chars: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for i in 1..=n {
        h.push(Message::Assistant(format!("checking thing{i}")));
        h.push(call(i));
        h.push(result(i, &"x".repeat(result_chars)));
    }
    h
}

fn digests(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            Message::User(t) if t.starts_with("## Earlier turns, folded") => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn approx_tokens(msgs: &[Message]) -> usize {
    msgs.iter()
        .map(|m| match m {
            Message::User(t) | Message::Assistant(t) => t.len(),
            Message::ToolCall { arguments, .. } => arguments.to_string().len(),
            Message::ToolResult { content, .. } => content.len(),
        })
        .sum::<usize>()
        / 4
}

#[test]
fn below_the_budget_it_is_exactly_conversational() {
    let st = State::new("fix it", "");
    let h = history(5, 100);
    let folded = Folded { budget_tokens: 48_000 };
    assert_eq!(folded.build(&st, &h), Conversational.build(&st, &h));
}

#[test]
fn over_the_budget_old_turns_become_digests_and_the_opening_stays() {
    let st = State::new("fix it", "");
    let h = history(200, 400); // ~20k tokens of results
    let out = Folded { budget_tokens: 4_000 }.build(&st, &h);
    assert!(matches!(&out[0], Message::User(t) if t == "Begin. fix it"));
    let d = digests(&out);
    assert!(!d.is_empty(), "expected at least one fold");
    assert!(matches!(&out[1], Message::User(t) if t.starts_with("## Earlier turns, folded")));
    let tail_tokens = approx_tokens(&out[1 + d.len()..]);
    assert!(tail_tokens <= 4_000 + 200, "verbatim tail {tail_tokens} tokens");
    assert!(out.len() < h.len());
}

#[test]
fn a_call_is_never_separated_from_its_result() {
    let st = State::new("fix it", "");
    let out = Folded { budget_tokens: 3_000 }.build(&st, &history(150, 300));
    let tail_start = 1 + digests(&out).len();
    assert!(
        !matches!(out[tail_start], Message::ToolResult { .. }),
        "the tail must not open with an orphaned result"
    );
    for (i, m) in out.iter().enumerate() {
        if let Message::ToolResult { id, .. } = m {
            match &out[i - 1] {
                Message::ToolCall { id: cid, .. } => assert_eq!(cid, id),
                other => panic!("result {id} not preceded by its call: {other:?}"),
            }
        }
    }
}

#[test]
fn earlier_folds_are_identical_on_the_next_turn() {
    let st = State::new("fix it", "");
    let f = Folded { budget_tokens: 3_000 };
    let short = f.build(&st, &history(120, 300));
    let long = f.build(&st, &history(160, 300));
    let (ds, dl) = (digests(&short), digests(&long));
    assert!(!ds.is_empty());
    assert!(dl.len() >= ds.len());
    assert_eq!(&dl[..ds.len()], &ds[..], "a later turn must not rewrite earlier digests");
    assert_eq!(short[0], long[0]);
}

#[test]
fn digests_summarise_actions_deterministically() {
    let st = State::new("fix it", "");
    let out = Folded { budget_tokens: 2_000 }.build(&st, &history(100, 300));
    let first = &digests(&out)[0];
    assert!(first.contains("`bash` `grep -rn thing1 src`"), "{first}");
    assert!(first.contains("you said: checking thing1"), "{first}");
    assert_eq!(
        out,
        Folded { budget_tokens: 2_000 }.build(&st, &history(100, 300)),
        "same history, same output"
    );
}

#[test]
fn recent_calls_keep_their_thought_signatures() {
    let st = State::new("fix it", "");
    let out = Folded { budget_tokens: 3_000 }.build(&st, &history(150, 300));
    let last_call = out
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::ToolCall { signature, .. } => Some(signature.clone()),
            _ => None,
        })
        .expect("a call in the tail");
    assert_eq!(last_call, Some("sig150".into()));
    assert_eq!(Folded::default().budget_tokens, 48_000);
    assert_eq!(Folded::default().name(), "folded");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_folded`. Expected: compile error, because `Folded` does not exist. (If `Message` does not implement `PartialEq`, stop and report; do not add derives.)

- [ ] **Step 3: Add the strategy.** Append to the end of `crates/superstate-agent/src/context.rs`:

```rust
/// An append-only conversation, bounded by folding.
///
/// `Conversational` keeps everything, which is what Gemini expects ("You MUST always resend all
/// `thought` blocks exactly as they were received") and what caches well, but it grows without
/// limit. The bounded strategies rebuild the prompt every turn, which drops thought signatures,
/// caches badly, and in every eval run lost the thread (superstate-eval FINDINGS 13.33-13.35).
///
/// `Folded` sends `[opening] [digest 1] … [digest k] [verbatim tail]`. Once the verbatim part
/// passes `budget_tokens`, its older half becomes one harness-written digest. Boundaries are
/// computed greedily from the start of history, so each depends only on the messages before it:
/// the next turn reproduces every earlier fold exactly, and between folds the output is pure
/// append. A cut never separates a tool call from its result.
#[derive(Debug, Clone)]
pub struct Folded {
    /// Approximate token budget (characters / 4) for the verbatim tail.
    pub budget_tokens: usize,
}

impl Default for Folded {
    fn default() -> Self {
        Self {
            budget_tokens: 48_000,
        }
    }
}

/// Characters / 4: a budget, not a billing figure.
fn approx_tokens(m: &Message) -> usize {
    let chars = match m {
        Message::User(t) | Message::Assistant(t) => t.len(),
        Message::ToolCall { arguments, .. } => arguments.to_string().len(),
        Message::ToolResult { content, .. } => content.len(),
    };
    chars / 4
}

fn clip(text: &str, max: usize) -> String {
    let one_line = text.lines().next().unwrap_or("").trim();
    if one_line.chars().count() > max {
        format!("{}...", one_line.chars().take(max).collect::<String>())
    } else {
        one_line.to_string()
    }
}

/// One harness-written line per folded item, from the same summarisers as the action ledger.
fn digest(folded: &[Message], first: usize, last: usize) -> Message {
    let mut text = format!(
        "## Earlier turns, folded (messages {first}-{last}; summarised by the harness)\n"
    );
    let mut i = 0;
    while i < folded.len() {
        match &folded[i] {
            Message::ToolCall {
                name, arguments, ..
            } => {
                let target = crate::run::extract_action_target(name, arguments);
                let summary = match folded.get(i + 1) {
                    Some(Message::ToolResult {
                        content, is_error, ..
                    }) => {
                        i += 1;
                        crate::run::extract_action_summary(name, content, *is_error)
                    }
                    _ => "no result".to_string(),
                };
                text.push_str(&format!("- `{name}` `{target}` -> {summary}\n"));
            }
            Message::Assistant(t) => text.push_str(&format!("- you said: {}\n", clip(t, 160))),
            Message::User(t) => text.push_str(&format!("- harness: {}\n", clip(t, 160))),
            Message::ToolResult { .. } => {}
        }
        i += 1;
    }
    Message::User(text)
}

impl ContextStrategy for Folded {
    fn build(&self, _state: &State, history: &[Message]) -> Vec<Message> {
        let Some((opening, rest)) = history.split_first() else {
            return Vec::new();
        };

        // Greedy boundaries from the start: each fold depends only on messages before it.
        let mut folds: Vec<(usize, usize)> = Vec::new();
        let mut seg_start = 0;
        let mut size = 0;
        for i in 0..rest.len() {
            size += approx_tokens(&rest[i]);
            if size <= self.budget_tokens {
                continue;
            }
            // Fold roughly the older half, cutting before a message that is not a result, so a
            // call is never separated from its result. No safe cut yet: keep accumulating.
            let target = seg_start + (i + 1 - seg_start) / 2;
            let lo = target.max(seg_start + 1);
            let Some(cut) = (lo..=i).find(|&j| !matches!(rest[j], Message::ToolResult { .. }))
            else {
                continue;
            };
            folds.push((seg_start, cut));
            size = rest[cut..=i].iter().map(approx_tokens).sum();
            seg_start = cut;
        }

        let mut out = vec![opening.clone()];
        for (a, b) in folds {
            // Message numbers are 1-based positions in the full history.
            out.push(digest(&rest[a..b], a + 2, b + 1));
        }
        out.extend_from_slice(&rest[seg_start..]);
        out
    }

    fn name(&self) -> &'static str {
        "folded"
    }
}
```

- [ ] **Step 4: Expose it on the CLI.**
  1. In `crates/superstate-cli/src/cli.rs`, in the `context` argument, replace `value_parser = ["state-only", "state-window", "conversational"]` with `value_parser = ["state-only", "state-window", "conversational", "folded"]`. Append ` or 'folded' (append-only chat, oldest half folded into digests past a budget)` to the doc comment line above it, before its final character if it ends with a closing parenthesis, keeping the line a single sentence.
  2. In `crates/superstate-cli/src/task_runner.rs`, after `let state_window = superstate_agent::context::StateWindow::default();` add `let folded = superstate_agent::context::Folded::default();`, and in the `match config.context_strategy.as_str()` add the arm `"folded" => &folded,` directly before `_ => &conversational,`.

- [ ] **Step 5: Run the new tests.** Run: `cargo test -p superstate-agent --test test_folded`. Expected: 6 passed.

- [ ] **Step 6: Check the CLI accepts it.** Run: `cargo run -q --bin superstate -- --help | grep -A2 -- "--context"`. Expected: the possible values include `folded`.

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 8: Commit.**

```bash
git add crates/superstate-agent/ crates/superstate-cli/
git commit --no-gpg-sign -m "feat(agent): folded context, append-only with harness-written digests past a budget"
```
