# Cache-Stable Prompt Prefix Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Make consecutive prompts share a long, growing prefix, so Gemini's automatic (implicit) cache applies.

**Why (superstate-eval FINDINGS §13.34):**
- **Caching barely happens.** Across every native run, at most ~150K input tokens were cached, against 2.5–5.8M fresh. The native harness costs 3.5–5× agy on the same task, and caching is the lever it has never used.
- **Gemini's rule:** implicit caching is automatic on Gemini 3.8 Flash, but only for a shared prompt prefix of at least **4,096 tokens** (ai.google.dev/gemini-api/docs/caching). It works best with large common content first and similar requests close together.
- **Our prefix is too short.** The fixed part of a request (system instruction, tools and task) measured about 838 tokens in a live call.
- **Our order is wrong for caching.** The state prompt runs Task → notes → working set → last failure → verification status → **action ledger (last)**. The working set changes almost every turn, so the shared prefix ends after the notes, and the one append-only section, the ledger, sits at the end, where it can't help.

**Design:** Reorder the `StateOnly` prompt, which `StateWindow` reuses, to **Task → Actions Taken So Far → Your state → Active Working Set → Last Test Failure / Verification Status**.
- The ledger is append-only, so turn N+1's prompt starts with turn N's task and full ledger, plus one new line.
- Once the ledger passes about 3,300 tokens (roughly 130 actions), each prefix clears 4,096 tokens and keeps growing. That covers the later half or more of a typical 330–380-turn run.
- The content is unchanged; only the order moves.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/cache-stable-prefix`. The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` is known to be timing-flaky: if it alone fails, re-run once and report both results.
- Existing tests must pass unmodified. None depends on section order.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Put the append-only ledger ahead of everything that changes

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`
- Test: `crates/superstate-agent/tests/test_prompt_prefix.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_prompt_prefix.rs`:

```rust
//! The state prompt keeps a stable, growing prefix, so Gemini's implicit cache (prefixes of at
//! least 4,096 tokens) can apply. The append-only action ledger comes before everything that
//! changes turn to turn.

use superstate_agent::context::{ContextStrategy, StateOnly, StateWindow};
use superstate_agent::state::{ActionEntry, State};
use superstate_agent::working_set::FileSnippet;
use superstate_provider::Message;

fn act(turn: u32, target: &str) -> ActionEntry {
    ActionEntry {
        turn,
        tool: "bash".into(),
        target: target.into(),
        summary: "exit 0".into(),
        is_error: false,
    }
}

fn snippet(path: &str, content: &str) -> FileSnippet {
    FileSnippet {
        path: path.into(),
        content: content.into(),
        start_line: None,
        end_line: None,
        last_accessed_turn: 1,
    }
}

fn prompt(strategy: &dyn ContextStrategy, st: &State) -> String {
    match &strategy.build(st, &[])[0] {
        Message::User(t) => t.clone(),
        other => panic!("expected the state prompt first, got {other:?}"),
    }
}

fn busy_state() -> State {
    let mut st = State::new("fix the numpy aliases", "pytest -q");
    st.runtime.actions = vec![act(1, "grep -rn n.float"), act(2, "pytest -q")];
    st.runtime.working_set.files = vec![snippet("a.py", "x = 1")];
    st.runtime.working_set.record_failure("E   AttributeError: n.float");
    st.agent
        .fields
        .insert("hypothesis".into(), serde_json::json!("numpy 2 removed n.float"));
    st
}

#[test]
fn the_ledger_comes_before_everything_that_changes() {
    let text = prompt(&StateOnly, &busy_state());
    let pos = |h: &str| text.find(h).unwrap_or_else(|| panic!("missing {h}:\n{text}"));
    let task = pos("## Task");
    let ledger = pos("## Actions Taken So Far");
    let notes = pos("## Your state");
    let working = pos("## Active Working Set");
    let failure = pos("## Last Test Failure");
    assert!(task < ledger, "{text}");
    assert!(ledger < notes, "{text}");
    assert!(notes < working, "{text}");
    assert!(working < failure, "{text}");
}

#[test]
fn the_next_turn_starts_with_this_turns_task_and_ledger() {
    let before = busy_state();
    let mut after = before.clone();
    // A new action, and every volatile section changes.
    after.runtime.actions.push(act(3, "edit a.py"));
    after.runtime.working_set.files = vec![snippet("b.py", "y = 2")];
    after.runtime.working_set.clear_failure();
    after
        .agent
        .fields
        .insert("next".into(), serde_json::json!("rerun pytest"));

    for strategy in [&StateOnly as &dyn ContextStrategy, &StateWindow::default()] {
        let a = prompt(strategy, &before);
        let b = prompt(strategy, &after);
        let stable = &a[..a.find("\n## Your state").expect("notes section")];
        assert!(
            b.starts_with(stable),
            "turn N+1 must start with turn N's task and ledger\n--- N ---\n{a}\n--- N+1 ---\n{b}"
        );
        assert!(stable.contains("grep -rn n.float") && stable.contains("pytest -q"));
    }
}

#[test]
fn with_no_actions_the_task_is_followed_by_the_notes() {
    let st = State::new("fix it", "");
    let text = prompt(&StateOnly, &st);
    assert!(!text.contains("## Actions Taken So Far"));
    assert!(text.find("## Task").unwrap() < text.find("## Your state").unwrap());
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_prompt_prefix`. Expected: `the_ledger_comes_before_everything_that_changes` and `the_next_turn_starts_with_this_turns_task_and_ledger` fail, because the ledger is currently last. `with_no_actions_the_task_is_followed_by_the_notes` passes. (If `State` does not implement `Clone`, stop and report; do not add derives.)

- [ ] **Step 3: Reorder the prompt.** In `crates/superstate-agent/src/context.rs`, in `impl ContextStrategy for StateOnly`, `fn build`:
  1. Replace

```rust
        let mut user_prompt = format!(
            "## Task\n{}\n\n## Your state\n{}\n",
            state.orchestrator.goal,
            serde_json::to_string_pretty(&state.agent.fields).unwrap_or_default(),
        );
```

     with

```rust
        // Order is for the cache as much as for the reader. Gemini caches a shared prompt prefix
        // of at least 4,096 tokens, so what is stable goes first: the task, then the append-only
        // action ledger, which turn N+1 repeats exactly with one more line. Everything that
        // changes turn to turn (notes, working set, failure, verification status) follows it.
        let mut user_prompt = format!("## Task\n{}\n", state.orchestrator.goal);

        // The chronological action ledger: episodic memory of tool outcomes without growing
        // conversation history. Append-only, which is what makes it cacheable.
        if !state.runtime.actions.is_empty() {
            user_prompt.push_str("\n## Actions Taken So Far\n");
            for action in &state.runtime.actions {
                let status_indicator = if action.is_error { " [ERROR]" } else { "" };
                user_prompt.push_str(&format!(
                    "- Turn {}: `{}` `{}` -> {}{}\n",
                    action.turn, action.tool, action.target, action.summary, status_indicator
                ));
            }
        }

        user_prompt.push_str(&format!(
            "\n## Your state\n{}\n",
            serde_json::to_string_pretty(&state.agent.fields).unwrap_or_default(),
        ));
```

  2. Delete the original ledger block further down: the comment lines `// When actions have been recorded, format the chronological action ledger.` and `// Provides episodic memory of tool outcomes without growing conversation history.`, and the whole `if !state.runtime.actions.is_empty() { … }` block that follows them, directly above `out.push(Message::User(user_prompt));`. The ledger must appear exactly once.

- [ ] **Step 4: Run the new tests.** Run: `cargo test -p superstate-agent --test test_prompt_prefix`. Expected: 3 passed.

- [ ] **Step 5: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 6: Commit.**

```bash
git add crates/superstate-agent/
git commit --no-gpg-sign -m "perf(agent): keep a stable prompt prefix so Gemini's implicit cache applies"
```
