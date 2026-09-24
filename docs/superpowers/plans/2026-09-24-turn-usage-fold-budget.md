# Per-Turn Usage and Fold Budget Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:**
- The run report records usage **per model call**, together with how the prompt changed since the previous call.
- The fold budget is settable (`--fold-budget N`).

**Why:** Fresh input is about half of a run's true cost: $0.60 of $1.33 on build-cython-ext (superstate-eval 13.42), at 4–6K fresh tokens per turn. Only the newest turn's tool results should be fresh. The rest is either new content or prefix-cache misses, and totals cannot tell those apart. The per-call record can:
- if the prompt was the previous one plus `new_chars` of new messages (`appended`), fresh input far above `new_chars / 4` is a cache miss;
- if the prompt was rewritten (a fold), a miss is expected.

That decides whether explicit Gemini caching is worth building. The fold budget is fixed at 48K today, and the next measurement needs a smaller-budget cell.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/turn-usage` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass. The **only** permitted edits to existing test files are the mechanical literal additions in Step 6. **If any other struct literal or exhaustive match breaks the build only because of the new fields, add the field mechanically there too and report it.**
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Per-turn usage in the run report, and `--fold-budget`

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Modify: `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/src/report.rs`, `crates/superstate-cli/src/repl.rs` (one literal)
- Modify (literals only, Step 6): `crates/superstate-cli/tests/test_run_report.rs`, `crates/superstate-cli/tests/test_task_runner.rs`, `crates/superstate-cli/tests/test_tdd_phases.rs`
- Test: `crates/superstate-agent/tests/test_turn_usage.rs`, `crates/superstate-cli/tests/test_fold_budget.rs` (create)

- [x] **Step 1: Write the failing tests.**

Create `crates/superstate-agent/tests/test_turn_usage.rs`:

```rust
//! Usage is recorded per model call, with how the prompt changed since the previous call, so
//! fresh input can be split into new content and cache misses.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{Conversational, ContextStrategy, Folded};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, RunReport, Runner};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};

struct Scripted {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
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

fn chars(m: &Message) -> usize {
    match m {
        Message::User(t) | Message::Assistant(t) => t.len(),
        Message::ToolCall { arguments, .. } => arguments.to_string().len(),
        Message::ToolResult { content, .. } => content.len(),
    }
}

fn usage(n: u64) -> Usage {
    Usage {
        input: 100 * n,
        cached: 10 * n,
        output: 5,
        thinking: 1,
    }
}

/// Three turns reading different 40-line files, then done.
fn run(context: &dyn ContextStrategy) -> (Outcome, RunReport, Vec<Request>) {
    let d = tempfile::tempdir().unwrap();
    let body = format!("{}\n", "x".repeat(50)).repeat(40);
    let mut script = Vec::new();
    for i in 0..3u64 {
        std::fs::write(d.path().join(format!("f{i}.txt")), &body).unwrap();
        script.push(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{i}"),
                name: "read".into(),
                arguments: json!({"path": format!("f{i}.txt")}),
                signature: None,
            }],
            usage: usage(i + 1),
            ..Default::default()
        });
    }
    script.push(Response {
        text: "done".into(),
        usage: usage(4),
        ..Default::default()
    });
    let p = Scripted {
        script: Mutex::new(script),
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: None,
            max_cost_usd: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (outcome, report) = runner.run(&mut State::new("read the files", "true"));
    let seen = p.seen.lock().unwrap().clone();
    (outcome, report, seen)
}

#[test]
fn every_turn_records_its_usage_and_whether_the_prompt_only_grew() {
    let (outcome, report, seen) = run(&Conversational);
    assert_eq!(outcome, Outcome::Verified);
    let u = &report.turn_usage;
    assert_eq!(u.iter().map(|t| t.turn).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert_eq!((u[1].input, u[1].cached, u[1].output, u[1].thinking), (200, 20, 5, 1));
    assert_eq!(report.usage.input, u.iter().map(|t| t.input).sum::<u64>());

    assert!(!u[0].appended, "the first call has nothing to extend");
    assert!(u[1..].iter().all(|t| t.appended), "conversational only appends");
    for k in 1..4 {
        assert_eq!(u[k].prompt_messages, seen[k].messages.len());
        let added: usize = seen[k].messages[seen[k - 1].messages.len()..]
            .iter()
            .map(chars)
            .sum();
        assert_eq!(u[k].new_chars, added, "turn {}", k + 1);
    }
    let whole: usize = seen[0].messages.iter().map(chars).sum();
    assert_eq!(u[0].new_chars, whole);
}

#[test]
fn a_fold_is_recorded_as_a_rewritten_prompt() {
    let (_, report, _) = run(&Folded { budget_tokens: 300 });
    assert!(
        report.turn_usage.iter().skip(1).any(|t| !t.appended),
        "{:?}",
        report.turn_usage
    );
}
```

Create `crates/superstate-cli/tests/test_fold_budget.rs`:

```rust
//! The fold budget is settable and recorded; per-turn usage reaches the run report.

use clap::Parser;
use superstate_agent::run::{Outcome, RunReport, TurnUsage};
use superstate_agent::state::State;
use superstate_cli::cli::Cli;
use superstate_cli::report::RunRecord;
use superstate_cli::task_runner::TaskConfig;

#[test]
fn the_fold_budget_and_turn_usage_reach_the_report() {
    let cli =
        Cli::try_parse_from(["superstate", "--context", "compact", "--fold-budget", "12000", "fix it"])
            .unwrap();
    assert_eq!(cli.fold_budget, Some(12000));
    let config = TaskConfig::from(&cli);
    assert_eq!(config.fold_budget, Some(12000));

    let report = RunReport {
        turn_usage: vec![TurnUsage {
            turn: 1,
            input: 10,
            cached: 5,
            output: 1,
            thinking: 0,
            prompt_messages: 1,
            appended: false,
            new_chars: 40,
        }],
        ..Default::default()
    };
    let r = RunRecord::from_run(
        &config,
        "gemini/gemini-3.8-flash",
        &Outcome::Verified,
        &report,
        &State::new("fix it", ""),
        std::time::Duration::from_secs(1),
    );
    assert_eq!(r.fold_budget, Some(12000));
    assert_eq!(r.turn_usage, report.turn_usage);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["fold_budget"], 12000);
    assert_eq!(v["turn_usage"][0]["new_chars"], 40);
    assert_eq!(v["turn_usage"][0]["appended"], false);
}

#[test]
fn the_fold_budget_defaults_to_unset() {
    let cli = Cli::try_parse_from(["superstate", "fix it"]).unwrap();
    assert_eq!(TaskConfig::from(&cli).fold_budget, None);
}
```

- [x] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_turn_usage` and `cargo test -p superstate --test test_fold_budget`. Expected: compile errors, because `turn_usage`, `TurnUsage` and `fold_budget` do not exist.

- [x] **Step 3: The runner.** In `crates/superstate-agent/src/run.rs`:
  1. Directly above `pub struct RunReport`, add:

```rust
/// One call of the main loop: what it cost, and how its prompt related to the previous call's.
/// `appended` means the prompt was the previous one plus `new_chars` of new messages, the only
/// case where a prefix cache can serve everything but the new part. Fresh input far above
/// `new_chars / 4` on an appended call is a cache miss; on a rewritten one (a fold) it is expected.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnUsage {
    pub turn: u32,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thinking: u64,
    pub prompt_messages: usize,
    pub appended: bool,
    pub new_chars: usize,
}

/// Characters a message contributes to a prompt, as the context strategies size them.
fn message_chars(m: &Message) -> usize {
    match m {
        Message::User(t) | Message::Assistant(t) => t.len(),
        Message::ToolCall { arguments, .. } => arguments.to_string().len(),
        Message::ToolResult { content, .. } => content.len(),
    }
}
```

  2. In `pub struct RunReport`, add as the last field:

```rust
    /// One entry per main-loop model call, in order.
    pub turn_usage: Vec<TurnUsage>,
```

  3. In `run_until`, directly **before** the line `last_sent = Some((req.messages.clone(), history.len()));`, add:

```rust
            let (appended, new_chars) = match &last_sent {
                Some((prev, _))
                    if req.messages.len() >= prev.len()
                        && req.messages[..prev.len()] == prev[..] =>
                {
                    (
                        true,
                        req.messages[prev.len()..].iter().map(message_chars).sum(),
                    )
                }
                _ => (false, req.messages.iter().map(message_chars).sum()),
            };
```

  4. In `run_until`, directly after the four `report.usage.<field> += resp.usage.<field>;` lines that follow the provider retry loop (not the ones in `review` or `fold_note`), add:

```rust
            report.turn_usage.push(TurnUsage {
                turn,
                input: resp.usage.input,
                cached: resp.usage.cached,
                output: resp.usage.output,
                thinking: resp.usage.thinking,
                prompt_messages: req.messages.len(),
                appended,
                new_chars,
            });
```

- [x] **Step 4: `--fold-budget`.**
  1. `crates/superstate-cli/src/cli.rs`, after the `resume` field:

```rust
    /// Approximate token budget for the verbatim tail before folding (folded and compact
    /// contexts; default 48000)
    #[arg(long)]
    pub fold_budget: Option<usize>,
```

  2. `crates/superstate-cli/src/task_runner.rs`:
     - In `TaskConfig`, after `resume`, add `/// Fold budget in approximate tokens (`--fold-budget`).` and `pub fold_budget: Option<usize>,`. In `From<&Cli>`, add `fold_budget: cli.fold_budget,`.
     - Replace `let folded = superstate_agent::context::Folded::default();` and `let compact = superstate_agent::context::Compact::default();` with:

```rust
    let folded = superstate_agent::context::Folded {
        budget_tokens: config
            .fold_budget
            .unwrap_or(superstate_agent::context::Folded::default().budget_tokens),
    };
    let compact = superstate_agent::context::Compact {
        budget_tokens: config
            .fold_budget
            .unwrap_or(superstate_agent::context::Compact::default().budget_tokens),
        ..superstate_agent::context::Compact::default()
    };
```

  3. `crates/superstate-cli/src/repl.rs`: in its `TaskConfig { ... }` literal, add `fold_budget: None,` after `resume: None,`.

- [x] **Step 5: The report.** In `crates/superstate-cli/src/report.rs`, in `RunRecord`:
  - After `cost_usd`, add `/// Fold budget given with `--fold-budget`, if any.`, `pub fold_budget: Option<usize>,`, then `/// Per-call usage (see `superstate_agent::run::TurnUsage`).` and `pub turn_usage: Vec<superstate_agent::run::TurnUsage>,`.
  - In `base`, add `fold_budget: config.fold_budget,` and `turn_usage: Vec::new(),`.
  - In `from_run`, add `turn_usage: report.turn_usage.clone(),`.

- [x] **Step 6: Literal additions in existing tests (mechanical).**
  - `crates/superstate-cli/tests/test_task_runner.rs` (4), `test_run_report.rs` (1), `test_tdd_phases.rs` (1): in each `TaskConfig { ... }` literal, add `fold_budget: None,` after `resume: None,`.
  - `crates/superstate-cli/tests/test_run_report.rs`: in the `RunReport { ... }` literal, add `turn_usage: Vec::new(),` as the last field.

- [x] **Step 7: Run the new tests.** Run: `cargo test -p superstate-agent --test test_turn_usage --test test_fold_notes --test test_resilience` and `cargo test -p superstate --test test_fold_budget --test test_run_report`. Expected: all pass (4 new).

- [x] **Step 8: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [x] **Step 9: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat: per-turn usage in the run report, and --fold-budget"
```
