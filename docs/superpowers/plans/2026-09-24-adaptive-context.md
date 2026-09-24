# Adaptive Fold Budget Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** A new context strategy, `adaptive`: `folded` whose budget starts small and grows when a fold proves too deep. Each turn's usage record carries the fold budget in force.

**Why (superstate-eval 13.44):** A fixed fold budget is either larger than the task needs to keep in view (wasted cached tokens) or smaller (the worker re-fetches what was folded, and a turn costs as much as 100–200K cached tokens). At 16K:
- `build-cython-ext` was 20% cheaper than at 48K;
- `scc` took 77% more turns, and re-reads of unchanged files went from about 1 to 4.3.

The re-fetch is the signal that a fold cut too deep.

**Design:**
- **Growth rule.** The budget starts at `min_tokens` (16K) and grows by `step_tokens` (8K), up to `max_tokens` (64K), each time the worker **re-fetches something only a fold removed from view**. That is a `read` of a path (any range), or an identical `grep`/`glob`, whose earlier matching call now lies in a folded segment, where:
  - no matching call is still in the verbatim tail;
  - for a read, the path has not been edited or written since.
  
  Re-running tests is legitimate and never counts.
- **Cache behaviour kept.** Growth is decided while scanning history in order, in the same greedy pass that places folds, so every fold still depends only on the messages before it. Folds stay prefix-stable, and the prompt still changes only when a fold happens.
- `ContextStrategy::fold_budget(history)` reports the budget in force (`None` for strategies that do not fold). The runner records it in `TurnUsage.fold_budget`.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/adaptive-context` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass. The **only** permitted edit to an existing test file is adding `fold_budget: None,` to the `TurnUsage { ... }` literal in `crates/superstate-cli/tests/test_fold_budget.rs`. **If any other struct literal or exhaustive match breaks the build only because of the new field, add it mechanically there too and report it.**
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: The `adaptive` context

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`, `crates/superstate-agent/src/run.rs`
- Modify: `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`
- Modify (literal only): `crates/superstate-cli/tests/test_fold_budget.rs`
- Test: `crates/superstate-agent/tests/test_adaptive.rs` (create)

- [x] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_adaptive.rs`:

```rust
//! `adaptive`: `folded` whose budget grows when the worker re-fetches something only a fold
//! removed from view. Folds stay prefix-stable and the prompt changes only when a fold happens.

use std::sync::Mutex;

use serde_json::{json, Value};
use superstate_agent::context::{Adaptive, Compact, ContextStrategy, Conversational, Folded};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Runner};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

const A: Adaptive = Adaptive {
    min_tokens: 2_000,
    step_tokens: 1_000,
    max_tokens: 4_000,
};

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

fn open() -> Vec<Message> {
    vec![Message::User("Begin. fix it".into())]
}

/// A 40-line output, about 520 tokens.
fn body() -> String {
    format!("{}\n", "o".repeat(51)).repeat(40)
}

/// `turns` distinct bash turns, never re-fetched.
fn filler(h: &mut Vec<Message>, from: usize, turns: usize) {
    for t in from..from + turns {
        h.push(call(t, "bash", json!({"command": format!("echo step {t}")})));
        h.push(res(t, "bash", &body()));
    }
}

fn read(h: &mut Vec<Message>, n: usize, path: &str) {
    h.push(call(n, "read", json!({ "path": path })));
    h.push(res(n, "read", &body()));
}

fn edit(h: &mut Vec<Message>, n: usize, path: &str) {
    h.push(call(n, "edit", json!({"path": path, "old": "a", "new": "b"})));
    h.push(res(n, "edit", "replaced 1 line"));
}

fn digests(out: &[Message]) -> usize {
    out.iter()
        .filter(|m| matches!(m, Message::User(t) if t.starts_with("## Earlier turns, folded")))
        .count()
}

#[test]
fn without_refetches_it_is_exactly_folded_at_its_minimum() {
    let mut h = open();
    read(&mut h, 0, "a.rs");
    filler(&mut h, 1, 12);
    let st = State::new("fix it", "");
    assert_eq!(A.build(&st, &h), Folded { budget_tokens: 2_000 }.build(&st, &h));
    assert_eq!(A.folds(&h), Folded { budget_tokens: 2_000 }.folds(&h));
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn re_reading_a_folded_file_grows_the_budget() {
    let mut h = open();
    read(&mut h, 0, "a.rs");
    filler(&mut h, 1, 12);
    assert!(A.folds(&h).first().is_some_and(|&(a, b)| a == 0 && b >= 2), "a.rs is folded");
    read(&mut h, 50, "a.rs");
    assert_eq!(A.fold_budget(&h), Some(3_000));
    // A ranged re-read of the same file counts too.
    let mut h2 = open();
    read(&mut h2, 0, "a.rs");
    filler(&mut h2, 1, 12);
    h2.push(call(50, "read", json!({"path": "a.rs", "start_line": 10, "end_line": 20})));
    h2.push(res(50, "read", "slice"));
    assert_eq!(A.fold_budget(&h2), Some(3_000));
}

#[test]
fn a_re_read_after_an_edit_is_not_a_refetch() {
    let mut h = open();
    read(&mut h, 0, "a.rs");
    filler(&mut h, 1, 12);
    edit(&mut h, 40, "a.rs");
    read(&mut h, 50, "a.rs");
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn a_repeat_of_a_call_still_in_view_is_not_a_refetch() {
    let mut h = open();
    filler(&mut h, 0, 12);
    read(&mut h, 40, "b.rs");
    read(&mut h, 41, "b.rs");
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn re_running_a_folded_command_is_not_a_refetch() {
    let mut h = open();
    h.push(call(0, "bash", json!({"command": "pytest -q"})));
    h.push(res(0, "bash", &body()));
    filler(&mut h, 1, 12);
    h.push(call(50, "bash", json!({"command": "pytest -q"})));
    h.push(res(50, "bash", &body()));
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn an_identical_folded_search_is_a_refetch() {
    let mut h = open();
    h.push(call(0, "grep", json!({"pattern": "fn parse", "path": "src"})));
    h.push(res(0, "grep", &body()));
    filler(&mut h, 1, 12);
    h.push(call(50, "grep", json!({"pattern": "fn parse", "path": "src"})));
    h.push(res(50, "grep", &body()));
    assert_eq!(A.fold_budget(&h), Some(3_000));
}

#[test]
fn growth_is_capped() {
    let mut h = open();
    for (n, p) in ["a.rs", "b.rs", "c.rs", "d.rs"].iter().enumerate() {
        read(&mut h, n, p);
    }
    filler(&mut h, 10, 12);
    for (n, p) in ["a.rs", "b.rs", "c.rs", "d.rs"].iter().enumerate() {
        read(&mut h, 60 + n, p);
    }
    assert_eq!(A.fold_budget(&h), Some(4_000));
}

#[test]
fn a_grown_budget_folds_less() {
    let mut h = open();
    for (n, p) in ["a.rs", "b.rs"].iter().enumerate() {
        read(&mut h, n, p);
    }
    filler(&mut h, 10, 8);
    for (n, p) in ["a.rs", "b.rs"].iter().enumerate() {
        read(&mut h, 60 + n, p);
    }
    filler(&mut h, 70, 8);
    let fixed = Folded { budget_tokens: 2_000 };
    assert!(A.fold_budget(&h) > Some(2_000));
    let st = State::new("fix it", "");
    let tail = |out: &[Message]| out.len() - 1 - digests(out);
    assert!(
        tail(&A.build(&st, &h)) > tail(&fixed.build(&st, &h)),
        "a larger budget keeps a longer verbatim tail"
    );
}

#[test]
fn the_prompt_only_changes_when_a_fold_happens() {
    let mut h = open();
    for t in 0..60 {
        let p = format!("f{}.rs", t % 5);
        match t % 4 {
            0 | 1 => read(&mut h, t, &p),
            2 => edit(&mut h, t, &p),
            _ => {
                h.push(call(t, "bash", json!({"command": "pytest -q"})));
                h.push(res(t, "bash", &body()));
            }
        }
    }
    let st = State::new("fix it", "");
    let mut folds_seen = 0;
    for n in 2..h.len() {
        let a = A.build(&st, &h[..n]);
        let b = A.build(&st, &h[..n + 1]);
        if b.len() >= a.len() && b[..a.len()] == a[..] {
            continue;
        }
        assert_ne!(digests(&a), digests(&b), "the prompt changed without a fold at n={n}");
        folds_seen += 1;
    }
    assert!(folds_seen > 0);
    assert!(A.fold_budget(&h) > Some(2_000), "this history re-reads folded files");
}

#[test]
fn every_folding_strategy_reports_its_budget() {
    let mut h = open();
    filler(&mut h, 0, 3);
    assert_eq!(Folded { budget_tokens: 300 }.fold_budget(&h), Some(300));
    assert_eq!(
        Compact {
            budget_tokens: 300,
            min_dedup_chars: 200
        }
        .fold_budget(&h),
        Some(300)
    );
    assert_eq!(Conversational.fold_budget(&h), None);
    assert_eq!(Adaptive::default().fold_budget(&h), Some(16_000));
    assert_eq!(
        (Adaptive::default().step_tokens, Adaptive::default().max_tokens),
        (8_000, 64_000)
    );
    assert_eq!(Adaptive::default().name(), "adaptive");
}

struct Scripted(Mutex<Vec<Response>>);

impl Provider for Scripted {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut s = self.0.lock().unwrap();
        Ok(if s.is_empty() {
            Response::default()
        } else {
            s.remove(0)
        })
    }
    fn model_id(&self) -> String {
        "scripted".into()
    }
}

#[test]
fn each_turn_records_the_budget_in_force() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted(Mutex::new(vec![Response {
        tool_calls: vec![ToolCall {
            id: "c0".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo hi"}),
            signature: None,
        }],
        ..Default::default()
    }]));
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &A,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 3,
            max_gate_attempts: 1,
            max_duration: None,
            max_cost_usd: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (_, report) = runner.run(&mut State::new("x", "true"));
    assert!(!report.turn_usage.is_empty());
    assert!(report.turn_usage.iter().all(|t| t.fold_budget == Some(2_000)));
}
```

- [x] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_adaptive`. Expected: compile errors, because `Adaptive`, `fold_budget` and `TurnUsage.fold_budget` do not exist.

- [x] **Step 3: Context.** In `crates/superstate-agent/src/context.rs`:
  1. In `pub trait ContextStrategy`, after the `folds` method, add:

```rust
    /// The fold budget in force after `history`, in approximate tokens, for strategies that fold.
    fn fold_budget(&self, _history: &[Message]) -> Option<usize> {
        None
    }
```

  2. In `struct FoldPlan`, add a field `budget: usize,` (doc: `/// The budget in force at the end of the scan.`).
  3. Replace the body of `Folded::plan` with `greedy_plan(rest, self.budget_tokens, None)`, keeping its signature and doc comment. Directly above `impl Folded {`, add:

```rust
/// Greedy fold boundaries over `rest`: each fold depends only on messages before it, so the next
/// turn reproduces every earlier fold exactly. With `grow = Some((step, max))` the budget grows by
/// `step` (to at most `max`) at each re-fetch of something already folded (`is_refetch`), decided
/// in the same in-order pass, so growth is prefix-stable too.
fn greedy_plan(rest: &[Message], start_budget: usize, grow: Option<(usize, usize)>) -> FoldPlan {
    let mut budget = start_budget;
    let mut folds: Vec<(usize, usize)> = Vec::new();
    let mut seg_start = 0;
    let mut size = 0;
    let mut last_trigger = None;
    for i in 0..rest.len() {
        if let Some((step, max)) = grow {
            if seg_start > 0 && is_refetch(rest, seg_start, i) {
                budget = (budget + step).min(max);
            }
        }
        size += approx_tokens(&rest[i]);
        if size <= budget {
            continue;
        }
        // Fold roughly the older half, cutting only between turn blocks (never after a call or
        // before a result), so a call is never separated from its result or from calls in the
        // same batch. No safe cut yet: keep accumulating.
        let target = seg_start + (i + 1 - seg_start) / 2;
        let lo = target.max(seg_start + 1);
        let Some(cut) = (lo..=i).find(|&j| {
            !matches!(rest[j], Message::ToolResult { .. })
                && !matches!(rest[j - 1], Message::ToolCall { .. })
        }) else {
            continue;
        };
        folds.push((seg_start, cut));
        size = rest[cut..=i].iter().map(approx_tokens).sum();
        seg_start = cut;
        last_trigger = Some(i);
    }
    FoldPlan {
        folds,
        seg_start,
        last_trigger,
        budget,
    }
}

/// What a call fetches, for re-fetch detection: a `read`'s path (any range), or a `grep`/`glob`'s
/// full arguments. Other tools (bash, edits) never count: re-running tests is legitimate.
fn fetch_key(m: &Message) -> Option<String> {
    let Message::ToolCall {
        name, arguments, ..
    } = m
    else {
        return None;
    };
    match name.as_str() {
        "read" => arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| format!("read:{p}")),
        "grep" | "glob" => Some(format!("{name}:{arguments}")),
        _ => None,
    }
}

/// Whether `rest[i]` re-fetches something only a fold removed from view: its fetch key matches a
/// call in `rest[..folded_end]`, no matching call is still in view (`rest[folded_end..i]`), and,
/// for a read, the file has not been edited or written since the folded read.
fn is_refetch(rest: &[Message], folded_end: usize, i: usize) -> bool {
    let Some(key) = fetch_key(&rest[i]) else {
        return false;
    };
    let matches = |m: &Message| fetch_key(m).as_deref() == Some(key.as_str());
    if rest[folded_end..i].iter().any(matches) {
        return false;
    }
    let Some(j) = (0..folded_end).rev().find(|&j| matches(&rest[j])) else {
        return false;
    };
    let Message::ToolCall {
        name, arguments, ..
    } = &rest[i]
    else {
        return false;
    };
    if name == "read" {
        let path = arguments.get("path").and_then(|v| v.as_str());
        let edited = rest[j + 1..i].iter().any(|m| {
            matches!(m, Message::ToolCall { name, arguments, .. }
                if (name == "edit" || name == "write")
                    && arguments.get("path").and_then(|v| v.as_str()) == path)
        });
        if edited {
            return false;
        }
    }
    true
}

/// The prompt for a fold plan: the opening, one digest per fold (with its fold note, if any),
/// then the verbatim tail.
fn render_folded(opening: &Message, rest: &[Message], plan: &FoldPlan, state: &State) -> Vec<Message> {
    let mut out = vec![opening.clone()];
    for &(a, b) in &plan.folds {
        // Message numbers are 1-based positions in the full history.
        out.push(digest(
            &rest[a..b],
            a + 2,
            b + 1,
            state.runtime.fold_notes.get(&b).map(String::as_str),
        ));
    }
    out.extend_from_slice(&rest[plan.seg_start..]);
    out
}
```

  4. In `impl ContextStrategy for Folded`, replace the body of `build` with:

```rust
        let Some((opening, rest)) = history.split_first() else {
            return Vec::new();
        };
        render_folded(opening, rest, &self.plan(rest), state)
```

     and add:

```rust
    fn fold_budget(&self, _history: &[Message]) -> Option<usize> {
        Some(self.budget_tokens)
    }
```

  5. In `impl ContextStrategy for Compact`, add the same `fold_budget` method (returning `Some(self.budget_tokens)`).
  6. After the `Compact` impls (at the end of the file's strategies), add:

```rust
/// `folded` with a budget that grows when a fold proves too deep.
///
/// A fixed budget is either larger than the task needs to keep in view (wasted cached tokens) or
/// smaller (the worker re-fetches what was folded, and a turn costs as much as 100-200K cached
/// tokens). At 16K, build-cython-ext was 20% cheaper than at 48K, but scc took 77% more turns and
/// re-read unchanged files four times as often (superstate-eval 13.44). Here the budget starts at
/// `min_tokens` and grows by `step_tokens`, up to `max_tokens`, each time the worker re-fetches
/// something only a fold removed from view (`is_refetch`). Growth is decided in the same in-order
/// pass that places folds, so folds stay prefix-stable and the prompt changes only at a fold.
#[derive(Debug, Clone)]
pub struct Adaptive {
    pub min_tokens: usize,
    pub step_tokens: usize,
    pub max_tokens: usize,
}

impl Default for Adaptive {
    fn default() -> Self {
        Self {
            min_tokens: 16_000,
            step_tokens: 8_000,
            max_tokens: 64_000,
        }
    }
}

impl Adaptive {
    fn plan(&self, rest: &[Message]) -> FoldPlan {
        greedy_plan(
            rest,
            self.min_tokens,
            Some((self.step_tokens, self.max_tokens)),
        )
    }
}

impl ContextStrategy for Adaptive {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        let Some((opening, rest)) = history.split_first() else {
            return Vec::new();
        };
        render_folded(opening, rest, &self.plan(rest), state)
    }

    fn folds(&self, history: &[Message]) -> Vec<(usize, usize)> {
        history
            .split_first()
            .map(|(_, rest)| self.plan(rest).folds)
            .unwrap_or_default()
    }

    fn fold_budget(&self, history: &[Message]) -> Option<usize> {
        Some(
            history
                .split_first()
                .map_or(self.min_tokens, |(_, rest)| self.plan(rest).budget),
        )
    }

    fn name(&self) -> &'static str {
        "adaptive"
    }
}
```

- [x] **Step 4: The runner.** In `crates/superstate-agent/src/run.rs`:
  1. In `pub struct TurnUsage`, add as the last field:

```rust
    /// The fold budget in force for this call, for strategies that fold.
    #[serde(default)]
    pub fold_budget: Option<usize>,
```

  2. In `run_until`, directly before the line `last_sent = Some((req.messages.clone(), history.len()));`, add `let fold_budget = self.context.fold_budget(&history);`.
  3. In the `report.turn_usage.push(TurnUsage { ... })` literal, add `fold_budget,` after `new_chars,`.

- [x] **Step 5: CLI.**
  1. `crates/superstate-cli/src/cli.rs`: add `"adaptive"` to the `--context` `value_parser` list. Append to its doc comment: ` or 'adaptive' (folded, with a budget that starts at --fold-budget or 16000 and grows when the worker re-fetches folded content)`.
  2. `crates/superstate-cli/src/task_runner.rs`: after the `let compact = ...;` statement, add:

```rust
    let adaptive = superstate_agent::context::Adaptive {
        min_tokens: config
            .fold_budget
            .unwrap_or(superstate_agent::context::Adaptive::default().min_tokens),
        ..superstate_agent::context::Adaptive::default()
    };
```

     and add `"adaptive" => &adaptive,` to the `match config.context_strategy.as_str()` arms, after `"compact" => &compact,`.
  3. `crates/superstate-cli/tests/test_fold_budget.rs`: in the `TurnUsage { ... }` literal, add `fold_budget: None,` after `new_chars: 40,`.

- [x] **Step 6: Run the new tests.** Run: `cargo test -p superstate-agent --test test_adaptive --test test_folded --test test_compact --test test_fold_notes --test test_turn_usage` and `cargo test -p superstate --test test_fold_budget`. Expected: all pass (11 new).

- [x] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [x] **Step 8: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): adaptive context — the fold budget grows when the worker re-fetches folded content"
```
