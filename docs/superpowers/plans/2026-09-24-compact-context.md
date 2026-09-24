# Compact Context Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** A `compact` context: `folded`, plus two **exact** reductions of tool output that only a harness observing every action can make safely.

**Why:**
- **Re-reading is most of the real cost.** A `conversational` run re-reads 16–20M cached input tokens (history × turns), about $1.40 at the cached rate, against about $0.75 of fresh input and output.
- **Tool outputs dominate history, and many are provably stale or duplicated.** In the four passing `build-cython-ext` runs, 22–52 reads and ~30 commands per run repeated an earlier call.
- **The research agrees.** Hiding old tool outputs while keeping the calls halved cost while matching LLM summarisation on SWE-bench Verified (JetBrains, "The Complexity Trap"). Reductions must be batched to keep the prompt prefix cacheable (TokenPilot, 2026).
- **Age isn't the best signal.** Research hides outputs by age; the harness knows which outputs are **superseded**, which is safer.

**Design (`Compact`, name `compact`; defaults: budget 48,000, epochs of 40 messages, dedupe outputs ≥ 200 chars):**
1. **Fold** exactly as `Folded`.
2. **Mask superseded outputs** in the verbatim tail. The call is kept; its output is replaced by a short marker. An output is superseded when a **later** call:
   - edits or writes the same `path`, for a `read`;
   - repeats the same `read` with **identical arguments**;
   - repeats an identical non-read call (`bash`, `grep`, `glob`, …).

   A read of a different range never supersedes. **Masks are applied only for completed epochs:** only superseding calls before the latest epoch boundary count. So the prompt prefix changes at most once per epoch (and at folds), not every turn.
3. **De-duplicate:** an output byte-identical to an **unmasked** output still in the tail is replaced by a reference to it. This is lossless, because the content is visible above. It is decided when the duplicate arrives, so it never rewrites earlier messages.
- Calls are never removed, so no result is ever orphaned; ids and names are unchanged.
- Pure over history, like `Folded`.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/folded-default-review-gate`. The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. A parallel task (the review gate) edits `run.rs`, `state.rs`, `report.rs` and other lines of `cli.rs` and `task_runner.rs`. Do not touch those files beyond the exact lines named here.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass unmodified. Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `Compact` and `--context compact`

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`, `crates/superstate-cli/src/cli.rs` (the `context` value list only), `crates/superstate-cli/src/task_runner.rs` (the context `match` only)
- Test: `crates/superstate-agent/tests/test_compact.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_compact.rs`:

```rust
//! `compact`: `folded` plus exact reductions of tool output. Superseded outputs are masked (in
//! completed epochs only), duplicates of visible outputs are referenced, and calls are never lost.

use serde_json::{json, Value};
use superstate_agent::context::{Compact, ContextStrategy, Conversational};
use superstate_agent::state::State;
use superstate_provider::Message;

fn call(n: usize, name: &str, args: Value) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: name.into(),
        arguments: args,
        signature: None,
    }
}

fn res(n: usize, name: &str, content: &str) -> Message {
    Message::ToolResult {
        id: format!("c{n}"),
        name: name.into(),
        content: content.into(),
        is_error: false,
    }
}

fn open() -> Message {
    Message::User("Begin. fix it".into())
}

fn content(m: &Message) -> &str {
    match m {
        Message::ToolResult { content, .. } => content,
        other => panic!("expected a tool result, got {other:?}"),
    }
}

fn compact(epoch: usize) -> Compact {
    Compact {
        budget_tokens: 1_000_000,
        epoch_messages: epoch,
        min_dedup_chars: 200,
    }
}

fn st() -> State {
    State::new("fix it", "")
}

#[test]
fn a_duplicate_of_a_visible_output_is_referenced_not_repeated() {
    let big = "x".repeat(500);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "read", json!({"path": "b.rs"})),
        res(2, "read", &"y".repeat(300)),
        call(3, "bash", json!({"command": "cat a.rs"})),
        res(3, "bash", &big),
    ];
    let out = compact(1_000).build(&st(), &h);
    assert_eq!(content(&out[2]), big, "the first occurrence stays");
    assert!(
        content(&out[6]).starts_with("[identical to the earlier output of `read` `a.rs` above"),
        "{}",
        content(&out[6])
    );
}

#[test]
fn short_outputs_are_never_deduplicated() {
    let h = vec![
        open(),
        call(1, "bash", json!({"command": "echo hi"})),
        res(1, "bash", "hi"),
        call(2, "bash", json!({"command": "echo hi again"})),
        res(2, "bash", "hi"),
    ];
    let out = compact(1_000).build(&st(), &h);
    assert_eq!(content(&out[4]), "hi");
}

#[test]
fn an_edit_supersedes_an_earlier_read_once_its_epoch_completes() {
    let big = "line\n".repeat(200);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        res(2, "edit", "replaced 1 line"),
        call(3, "bash", json!({"command": "ls"})),
        res(3, "bash", "a.rs"),
    ];
    let out = compact(4).build(&st(), &h);
    assert!(content(&out[2]).contains("a.rs changed later"), "{}", content(&out[2]));
    assert!(matches!(&out[1], Message::ToolCall { name, .. } if name == "read"), "the call is kept");
}

#[test]
fn nothing_is_masked_before_its_epoch_completes() {
    let big = "line\n".repeat(200);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        res(2, "edit", "replaced 1 line"),
    ];
    let out = compact(100).build(&st(), &h);
    assert_eq!(content(&out[2]), big);
}

#[test]
fn an_identical_command_rerun_supersedes_the_earlier_output() {
    let h = vec![
        open(),
        call(1, "bash", json!({"command": "pytest -q"})),
        res(1, "bash", &format!("FAILED {}", "f".repeat(300))),
        call(2, "bash", json!({"command": "pytest -q"})),
        res(2, "bash", &format!("passed {}", "p".repeat(300))),
        call(3, "read", json!({"path": "z.rs"})),
        res(3, "read", "z"),
    ];
    let out = compact(4).build(&st(), &h);
    assert!(content(&out[2]).contains("later run of the same call"), "{}", content(&out[2]));
    assert!(content(&out[4]).starts_with("passed"), "the latest run stays");
}

#[test]
fn a_read_of_a_different_range_does_not_supersede() {
    let big = "line\n".repeat(200);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "read", json!({"path": "a.rs", "start_line": 10, "end_line": 20})),
        res(2, "read", "slice"),
        call(3, "bash", json!({"command": "ls"})),
        res(3, "bash", "a.rs"),
    ];
    let out = compact(4).build(&st(), &h);
    assert_eq!(content(&out[2]), big);
}

#[test]
fn the_prompt_only_changes_at_epoch_boundaries() {
    let c = Compact {
        budget_tokens: 1_000_000,
        epoch_messages: 8,
        min_dedup_chars: 20,
    };
    let mut h = vec![open()];
    for t in 0..30 {
        let (name, args) = match t % 3 {
            0 => ("read", json!({"path": format!("f{}.rs", t % 4)})),
            1 => ("edit", json!({"path": format!("f{}.rs", t % 4), "old": "a", "new": "b"})),
            _ => ("bash", json!({"command": "pytest -q"})),
        };
        h.push(call(t, name, args));
        h.push(res(t, name, &format!("output of {name} {}", "z".repeat(40 + t % 2))));
    }
    let mut changes = 0;
    for n in 2..h.len() {
        let a = compact_build(&c, &h[..n]);
        let b = compact_build(&c, &h[..n + 1]);
        if b.len() >= a.len() && b[..a.len()] == a[..] {
            continue;
        }
        assert_ne!((n - 1) / 8, n / 8, "the prompt changed mid-epoch at n={n}");
        changes += 1;
    }
    assert!(changes > 0, "masks should apply at some epoch boundary");
}

fn compact_build(c: &Compact, h: &[Message]) -> Vec<Message> {
    c.build(&st(), h)
}

#[test]
fn with_nothing_to_reduce_it_is_exactly_conversational() {
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &"a".repeat(300)),
        call(2, "read", json!({"path": "b.rs"})),
        res(2, "read", &"b".repeat(300)),
    ];
    assert_eq!(compact(4).build(&st(), &h), Conversational.build(&st(), &h));
    assert_eq!(Compact::default().epoch_messages, 40);
    assert_eq!(Compact::default().budget_tokens, 48_000);
    assert_eq!(Compact::default().min_dedup_chars, 200);
    assert_eq!(Compact::default().name(), "compact");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_compact`. Expected: compile error, because `Compact` does not exist.

- [ ] **Step 3: Add the strategy.** Append to the end of `crates/superstate-agent/src/context.rs`:

```rust
/// `folded`, plus two exact reductions of tool output that only the harness can make safely,
/// because it observes every action.
///
/// Re-reading history is most of a long run's real cost (16–20M cached input tokens on
/// build-cython-ext), and tool outputs dominate history. Hiding old outputs while keeping the calls
/// halved cost at no loss of solve rate on SWE-bench Verified (JetBrains, "The Complexity Trap"),
/// but hiding by age can drop something still relevant. The harness knows which outputs are
/// **superseded**: a read of a file edited since, or a call repeated since with identical
/// arguments. Those are masked; outputs identical to one still visible are referenced. Masks apply
/// only for completed epochs, so the cacheable prefix changes once per epoch rather than every
/// turn (TokenPilot, 2026). Calls are never removed, so no result is ever orphaned.
#[derive(Debug, Clone)]
pub struct Compact {
    /// Approximate token budget for the verbatim tail, as in `Folded`.
    pub budget_tokens: usize,
    /// Supersession is applied only for completed epochs of this many tail messages.
    pub epoch_messages: usize,
    /// Outputs shorter than this are never replaced by a duplicate reference.
    pub min_dedup_chars: usize,
}

impl Default for Compact {
    fn default() -> Self {
        Self {
            budget_tokens: 48_000,
            epoch_messages: 40,
            min_dedup_chars: 200,
        }
    }
}

/// The call that produced the result at `r`: calls and results of a turn are recorded as a block
/// (calls, then results, in call order), so the k-th result of a run pairs with the k-th call of
/// the run before it. Also correct for the older interleaved layout (a block of one).
fn call_for_result(h: &[Message], r: usize) -> Option<&Message> {
    let mut results_start = r;
    while results_start > 0 && matches!(h[results_start - 1], Message::ToolResult { .. }) {
        results_start -= 1;
    }
    let k = r - results_start;
    let mut calls_start = results_start;
    while calls_start > 0 && matches!(h[calls_start - 1], Message::ToolCall { .. }) {
        calls_start -= 1;
    }
    h[calls_start..results_start].get(k)
}

/// Why the output at `r` (produced by `name(args)`) is superseded by a later call before `cutoff`.
fn superseded(
    h: &[Message],
    r: usize,
    cutoff: usize,
    name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    let path = args.get("path").and_then(|v| v.as_str());
    for m in &h[r + 1..cutoff] {
        let Message::ToolCall {
            name: later,
            arguments: later_args,
            ..
        } = m
        else {
            continue;
        };
        let later_path = later_args.get("path").and_then(|v| v.as_str());
        match (name, later.as_str()) {
            ("read", "edit" | "write") if path.is_some() && path == later_path => {
                return Some(format!("superseded: {} changed later", path.unwrap_or_default()));
            }
            ("read", "read") if args == later_args => {
                return Some(format!(
                    "superseded by a later identical read of {}",
                    path.unwrap_or_default()
                ));
            }
            (n, l) if n == l && !matches!(n, "read" | "edit" | "write" | "note") && args == later_args => {
                return Some("superseded by a later run of the same call".to_string());
            }
            _ => {}
        }
    }
    None
}

impl ContextStrategy for Compact {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        let mut out = Folded {
            budget_tokens: self.budget_tokens,
        }
        .build(state, history);
        if out.is_empty() {
            return out;
        }
        let digests = out
            .iter()
            .skip(1)
            .take_while(|m| matches!(m, Message::User(t) if t.starts_with("## Earlier turns, folded")))
            .count();
        let tail_out = 1 + digests;
        let tail_len = out.len() - tail_out;
        let tail_start = history.len() - tail_len;
        let epoch = self.epoch_messages.max(1);
        let cutoff = tail_start + (tail_len / epoch) * epoch;

        // Superseded outputs, judged only by calls inside completed epochs.
        let mut masked = vec![false; history.len()];
        for r in tail_start..cutoff {
            if !matches!(history[r], Message::ToolResult { .. }) {
                continue;
            }
            let Some(Message::ToolCall {
                name, arguments, ..
            }) = call_for_result(history, r)
            else {
                continue;
            };
            if let Some(reason) = superseded(history, r, cutoff, name, arguments) {
                if let Message::ToolResult { content, .. } = &mut out[tail_out + (r - tail_start)] {
                    *content = format!("[{reason}; output elided by superstate]");
                }
                masked[r] = true;
            }
        }

        // Exact duplicates of an output still visible above.
        for r in tail_start..history.len() {
            let Message::ToolResult { content: here, .. } = &history[r] else {
                continue;
            };
            if masked[r] || here.len() < self.min_dedup_chars {
                continue;
            }
            let earlier = (tail_start..r).find(|&q| {
                !masked[q]
                    && matches!(&history[q], Message::ToolResult { content, .. } if content == here)
            });
            let Some(q) = earlier else {
                continue;
            };
            let what = match call_for_result(history, q) {
                Some(Message::ToolCall {
                    name, arguments, ..
                }) => format!("`{name}` `{}`", crate::run::extract_action_target(name, arguments)),
                _ => "an earlier call".to_string(),
            };
            if let Message::ToolResult { content, .. } = &mut out[tail_out + (r - tail_start)] {
                *content = format!("[identical to the earlier output of {what} above; elided by superstate]");
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "compact"
    }
}
```

- [ ] **Step 4: Expose it on the CLI.**
  1. In `crates/superstate-cli/src/cli.rs`, in the `context` argument's `value_parser` list, append `"compact"` after `"folded"`, and append ` or 'compact' (folded, plus superseded outputs masked and duplicates referenced)` to the end of its doc comment line (after the existing final clause, keeping one sentence).
  2. In `crates/superstate-cli/src/task_runner.rs`, directly after `let folded = superstate_agent::context::Folded::default();`, add `let compact = superstate_agent::context::Compact::default();`, and add the arm `"compact" => &compact,` directly before `"folded" => &folded,`.

- [ ] **Step 5: Run the new tests.** Run: `cargo test -p superstate-agent --test test_compact`. Expected: 8 passed.

- [ ] **Step 6: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 7: Commit.**

```bash
git add crates/superstate-agent/ crates/superstate-cli/src/cli.rs crates/superstate-cli/src/task_runner.rs
git commit --no-gpg-sign -m "feat(agent): compact context masks superseded outputs and references duplicates"
```
