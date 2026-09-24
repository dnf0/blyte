# Resilience Implementation Plan: server retry hints, cost cap, journal and resume

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** A run survives what it can survive and reports honestly what it can't:
1. **Retry hints:** a rate-limited call waits as long as the server asked.
2. **Cost cap:** a run stops itself at a dollar limit.
3. **Journal and resume:** the run so far is saved before every model call, so a crash, an exhausted budget or retries running out can be resumed instead of restarted.

**Why ("5 resilience patterns for AI agents", thetshaped.dev, applied to superstate):**
- *Retry the step, not the run.* Superstate retries 429/5xx with fixed backoff and ignores the delay Gemini puts in a 429 (`"retryDelay": "37s"`), so it retries too early and burns attempts.
- *Checkpoint after every step.* Only phase ends are checkpointed (git commits). A run that dies at turn 150 loses its whole conversation.
- *Bound the loop on tokens too.* Turns and wall clock are bounded, but money is not, although true cost is now known (superstate-eval 13.39).

**Design:**
- `superstate_provider::pricing(model_id)` gives list prices. `Pricing::cost(&Usage)` bills thinking as output, which is how Gemini bills it.
- `Budget.max_cost_usd` is checked before each turn, per `run_until` call (per phase in a phased run). A cap on a model with no known price is an immediate `Outcome::Error`, never silently unenforced.
- **Retry wait** = max(backoff, server hint), with the hint capped at 120s. The deadline rule is unchanged.
- **Journal.** `Event::Checkpoint { history, state }` is emitted before every model call and when the turn budget runs out. The CLI writes it atomically to `--journal PATH`. `Runner::resume(state, history)` continues with a fresh budget; `--resume PATH` loads a journal. A checkpoint is taken between turns, so a recorded tool result is never re-executed.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/resilience` (from `feat/fold-notes`, which it builds on). Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. Task 2 builds on Task 1.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass. The **only** permitted edits to existing test files are the mechanical literal additions each task names.
- Commit with `--no-gpg-sign` and the exact message in each task.

---

### Task 1: Pricing, cost cap, retry hints, checkpoints and resume (agent and provider)

**Files:**
- Modify: `crates/superstate-provider/src/lib.rs`, `crates/superstate-agent/src/run.rs`, `crates/superstate-cli/src/repl.rs` (one match arm)
- Modify (literals only): every file containing a `Budget { ... }` literal (Step 5)
- Test: `crates/superstate-agent/tests/test_resilience.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_resilience.rs`:

```rust
//! Resilience: server retry hints, a cost cap, and checkpoints a run can be resumed from.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use superstate_agent::context::Conversational;
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{
    retry_delay, retry_wait, server_retry_hint, Budget, Event, Outcome, Runner,
};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{
    pricing, Message, Provider, ProviderError, Request, Response, ToolCall, Usage,
};

struct Scripted {
    model: String,
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Scripted {
    fn new(model: &str, script: Vec<Response>) -> Self {
        Self {
            model: model.into(),
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }
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
        self.model.clone()
    }
}

fn bash(id: &str, command: &str, usage: Usage) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: "bash".into(),
            arguments: json!({ "command": command }),
            signature: None,
        }],
        usage,
        ..Default::default()
    }
}

fn budget(max_turns: u32, max_cost_usd: Option<f64>) -> Budget {
    Budget {
        max_turns,
        max_gate_attempts: 1,
        max_duration: None,
        max_cost_usd,
    }
}

fn api(status: u16, body: &str) -> ProviderError {
    ProviderError::Api {
        status,
        body: body.into(),
    }
}

#[test]
fn the_server_retry_hint_is_honoured_and_bounded() {
    let body = r#"{"error": {"code": 429, "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "37s"}]}}"#;
    assert_eq!(server_retry_hint(&api(429, body)), Some(Duration::from_secs(37)));
    assert_eq!(retry_wait(&api(429, body), 1), Duration::from_secs(37));
    // A hint shorter than the backoff does not shorten it.
    let short = r#"{"retryDelay": "1s"}"#;
    assert_eq!(retry_wait(&api(429, short), 3), retry_delay(3));
    // No hint: the backoff.
    assert_eq!(retry_wait(&api(503, "overloaded"), 2), retry_delay(2));
    assert_eq!(server_retry_hint(&ProviderError::Transport("reset".into())), None);
    // An absurd hint is capped.
    let long = r#"{"retryDelay": "900s"}"#;
    assert_eq!(server_retry_hint(&api(429, long)), Some(Duration::from_secs(120)));
}

#[test]
fn gemini_flash_is_priced_with_thinking_billed_as_output() {
    let p = pricing("gemini/gemini-3.8-flash").expect("a known price");
    let u = Usage {
        input: 1_000_000,
        cached: 1_000_000,
        output: 100_000,
        thinking: 100_000,
    };
    assert!((p.cost(&u) - 1.575).abs() < 1e-9, "{}", p.cost(&u));
    assert_eq!(pricing("scripted"), None);
}

#[test]
fn a_run_stops_itself_at_its_cost_cap() {
    let d = tempfile::tempdir().unwrap();
    let million = Usage {
        input: 1_000_000,
        ..Default::default()
    };
    // $0.75 per turn; the cap of $2 is reached after three turns.
    let script = (0..6).map(|i| bash(&format!("c{i}"), "echo hi", million)).collect();
    let p = Scripted::new("gemini/gemini-3.8-flash", script);
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(10, Some(2.0)),
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (outcome, report) = runner.run(&mut State::new("loop", "true"));
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(p.seen.lock().unwrap().len(), 3);
    assert_eq!(report.usage.input, 3_000_000);
}

#[test]
fn a_cost_cap_without_a_known_price_is_an_error_not_ignored() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted::new("scripted", vec![]);
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(10, Some(1.0)),
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (outcome, _) = runner.run(&mut State::new("x", "true"));
    assert!(
        matches!(&outcome, Outcome::Error(e) if e.contains("needs a known price")),
        "{outcome:?}"
    );
    assert!(p.seen.lock().unwrap().is_empty());
}

#[test]
fn a_run_resumes_from_its_last_checkpoint_without_rerunning_tools() {
    let d = tempfile::tempdir().unwrap();
    let saved: Mutex<Option<(Vec<Message>, State)>> = Mutex::new(None);
    let on_event = |e: Event<'_>| {
        if let Event::Checkpoint { history, state } = e {
            *saved.lock().unwrap() = Some((history.to_vec(), state.clone()));
        }
    };

    // First run: one turn that appends to a log, then the turn budget runs out.
    let p1 = Scripted::new(
        "scripted",
        vec![bash("c0", "echo ran >> log.txt && echo logged", Usage::default())],
    );
    let r1 = Runner {
        provider: &p1,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(1, None),
        rollback: false,
        loop_breaker: false,
        on_event: Some(&on_event),
    };
    let mut st = State::new("log once", "test -f log.txt");
    let (outcome, _) = r1.run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    let (history, state) = saved.lock().unwrap().take().expect("a checkpoint");
    assert!(
        history.iter().any(|m| matches!(m, Message::ToolResult { content, .. } if content.contains("logged"))),
        "the final checkpoint includes the last turn's results: {history:?}"
    );

    // Resume: the model sees the saved history, and nothing already run is run again.
    let p2 = Scripted::new(
        "scripted",
        vec![Response {
            text: "done".into(),
            ..Default::default()
        }],
    );
    let r2 = Runner {
        provider: &p2,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(5, None),
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let mut st2 = state;
    let (outcome, _) = r2.resume(&mut st2, history.clone());
    assert_eq!(outcome, Outcome::Verified);
    let first = p2.seen.lock().unwrap()[0].clone();
    assert_eq!(&first.messages[..history.len()], &history[..]);
    let log = std::fs::read_to_string(d.path().join("log.txt")).unwrap();
    assert_eq!(log.lines().count(), 1, "the tool ran exactly once: {log:?}");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_resilience`. Expected: compile errors, because `retry_wait`, `server_retry_hint`, `pricing`, `max_cost_usd`, `Event::Checkpoint` and `Runner::resume` do not exist.

- [ ] **Step 3: Pricing.** In `crates/superstate-provider/src/lib.rs`, directly after the `pub struct Usage { ... }` definition, add:

```rust
/// List prices in USD per million tokens: fresh input, cached input, and output. Thinking is
/// billed as output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
}

impl Pricing {
    /// What `u` costs. `Usage.input` is fresh input only; cached input is counted separately.
    pub fn cost(&self, u: &Usage) -> f64 {
        (u.input as f64 * self.input
            + u.cached as f64 * self.cached
            + (u.output + u.thinking) as f64 * self.output)
            / 1e6
    }
}

/// The list price for a model id, if known. A cost cap on a model with no known price is an
/// error, never silently unenforced.
pub fn pricing(model_id: &str) -> Option<Pricing> {
    // Gemini 3.8 Flash, list price through 2026-12-31.
    if model_id.contains("gemini-3.8-flash") {
        return Some(Pricing {
            input: 0.75,
            cached: 0.075,
            output: 3.75,
        });
    }
    None
}
```

- [ ] **Step 4: Runner.** In `crates/superstate-agent/src/run.rs`:
  1. In `pub struct Budget`, after `max_duration`, add:

```rust
    /// Dollar limit at list price (`superstate_provider::pricing`), checked before each turn of a
    /// `run_until` call (so per phase in a phased run). Requires a model with a known price.
    pub max_cost_usd: Option<f64>,
```

     Do **not** edit `impl Default for Budget` by hand: Step 5's command adds `max_cost_usd: None,` there along with every other literal.
  2. In `pub enum Event<'e>`, after the `RolledBack` variant, add:

```rust
    /// The run so far, emitted before each model call and when the turn budget runs out. Taken
    /// between turns, so `Runner::resume` from it never re-executes a recorded tool result.
    Checkpoint {
        history: &'e [Message],
        state: &'e State,
    },
```

  3. Directly after the `retry_delay` function, add:

```rust
/// How long the server asked us to wait, if it said: Gemini puts `"retryDelay": "37s"` in a
/// 429's body. Capped at 120 seconds.
pub fn server_retry_hint(e: &ProviderError) -> Option<Duration> {
    let ProviderError::Api { body, .. } = e else {
        return None;
    };
    let key = "\"retryDelay\"";
    let rest = &body[body.find(key)? + key.len()..];
    let value = &rest[rest.find('"')? + 1..];
    let secs: f64 = value[..value.find('s')?].trim().parse().ok()?;
    Some(Duration::from_secs_f64(secs.max(0.0)).min(Duration::from_secs(120)))
}

/// Wait before retry `attempt` (1-based): the backoff, or the server's hint when it is longer.
pub fn retry_wait(e: &ProviderError, attempt: u32) -> Duration {
    let backoff = retry_delay(attempt);
    server_retry_hint(e).map_or(backoff, |hint| hint.max(backoff))
}
```

  4. Change `fn run_until(&self, state: &mut State, deadline: Option<std::time::Instant>) -> (Outcome, RunReport)` to take a third parameter `resumed: Option<Vec<Message>>`. Update both existing call sites to pass `None` (in `run` and in `run_phases`).
  5. In `run_until`:
     - Replace `let mut history: Vec<Message> = vec![Message::User(format!("Begin. {}", state.orchestrator.goal))];` with:

```rust
        let resuming = resumed.is_some();
        let mut history: Vec<Message> = resumed.unwrap_or_else(|| {
            vec![Message::User(format!("Begin. {}", state.orchestrator.goal))]
        });
```

     - Make the existing `state.runtime.fold_notes.clear();` conditional: `if !resuming { state.runtime.fold_notes.clear(); }` (a resumed history keeps its fold notes).
     - Directly before `for turn in 1..=self.budget.max_turns {`, add:

```rust
        let price = superstate_provider::pricing(&self.provider.model_id());
        if self.budget.max_cost_usd.is_some() && price.is_none() {
            return (
                Outcome::Error(format!(
                    "a cost cap needs a known price; none for model {}",
                    self.provider.model_id()
                )),
                report,
            );
        }
```

     - As the first statement inside the turn loop (before the deadline check), add:

```rust
            self.emit(Event::Checkpoint {
                history: &history,
                state,
            });
```

     - Directly after the deadline check's `if` block, add:

```rust
            if let (Some(cap), Some(p)) = (self.budget.max_cost_usd, price) {
                if p.cost(&report.usage) >= cap {
                    return (Outcome::BudgetExhausted, report);
                }
            }
```

     - In the retry loop, replace `let wait = retry_delay(attempt);` with `let wait = retry_wait(&e, attempt);`.
     - At the end of `run_until`, directly before the final `(Outcome::BudgetExhausted, report)`, add:

```rust
        self.emit(Event::Checkpoint {
            history: &history,
            state,
        });
```

  6. Directly after `pub fn run(...) { ... }`, add:

```rust
    /// Continue a run from a checkpoint (`Event::Checkpoint`) with a fresh budget. Tool results
    /// already in `history` are never re-executed.
    pub fn resume(&self, state: &mut State, history: Vec<Message>) -> (Outcome, RunReport) {
        let deadline = self
            .budget
            .max_duration
            .map(|d| std::time::Instant::now() + d);
        self.run_until(state, deadline, Some(history))
    }
```

  7. `crates/superstate-cli/src/repl.rs`, in `format_event`, add the arm `superstate_agent::run::Event::Checkpoint { .. } => String::new(),`.

- [ ] **Step 5: `Budget` literals (mechanical).** Run from the repo root:

```bash
grep -rl "max_duration:" crates --include='*.rs' | xargs perl -0pi -e 's/(\n([ \t]*)max_duration: [^\n]*,\n)/$1$2max_cost_usd: None,\n/g'
```

  This adds `max_cost_usd: None,` after every `max_duration: ...,` literal line, including `impl Default for Budget` and `task_runner.rs` (Task 2 changes that one). The declaration `pub max_duration` is not matched. Run `git diff --stat` and confirm that only `Budget` literals changed besides your Step 3–4 edits.

- [ ] **Step 6: Run the new tests.** Run: `cargo test -p superstate-agent --test test_resilience --test test_provider_retry --test test_phases --test test_fold_notes`. Expected: all pass (5 new).

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 8: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): server retry hints, cost cap, checkpoints and resume"
```

---

### Task 2: CLI: `--max-cost-usd`, `--journal`, `--resume`, and cost in the run report

**Files:**
- Create: `crates/superstate-cli/src/journal.rs`
- Modify: `crates/superstate-cli/src/lib.rs`, `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/src/report.rs`, `crates/superstate-cli/src/main.rs`
- Modify (literals only): `crates/superstate-cli/src/repl.rs` (1), `crates/superstate-cli/tests/test_task_runner.rs` (4), `test_run_report.rs` (1), `test_tdd_phases.rs` (1): each `TaskConfig { ... }` literal
- Test: `crates/superstate-cli/tests/test_journal.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-cli/tests/test_journal.rs`:

```rust
//! A saved run can be reloaded; the CLI exposes the cost cap, journal and resume.

use clap::Parser;
use superstate_agent::state::State;
use superstate_cli::cli::Cli;
use superstate_cli::journal;
use superstate_provider::Message;

#[test]
fn a_journal_round_trips_and_is_written_atomically() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("run.journal.json");
    let mut state = State::new("fix it", "cargo test");
    state.runtime.fold_notes.insert(4, "Established:\n- x\n".into());
    let history = vec![
        Message::User("Begin. fix it".into()),
        Message::Assistant("looking".into()),
    ];
    journal::write(&path, &state, &history).unwrap();
    journal::write(&path, &state, &history).unwrap();
    let j = journal::read(&path).unwrap();
    assert_eq!(j.schema_version, journal::SCHEMA_VERSION);
    assert_eq!(j.state, state);
    assert_eq!(j.history, history);
    let leftovers: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
}

#[test]
fn reading_a_missing_journal_is_an_error() {
    assert!(journal::read(std::path::Path::new("/nonexistent/j.json")).is_err());
}

#[test]
fn the_cli_parses_cost_cap_journal_and_resume() {
    let cli = Cli::try_parse_from([
        "superstate",
        "--max-cost-usd",
        "2.5",
        "--journal",
        "j.json",
        "--resume",
        "old.json",
    ])
    .unwrap();
    assert_eq!(cli.max_cost_usd, Some(2.5));
    assert_eq!(cli.journal.as_deref(), Some(std::path::Path::new("j.json")));
    assert_eq!(cli.resume.as_deref(), Some(std::path::Path::new("old.json")));
    assert!(Cli::try_parse_from(["superstate", "--resume", "old.json", "--tdd"]).is_err());
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate --test test_journal`. (If the package name differs, use the one in `crates/superstate-cli/Cargo.toml`.) Expected: compile errors, because `journal`, `max_cost_usd`, `journal` and `resume` do not exist.

- [ ] **Step 3: The journal module.** Create `crates/superstate-cli/src/journal.rs`:

```rust
//! A saved run: the history and state at the last checkpoint, enough to `--resume` it.

use std::path::Path;

use serde::{Deserialize, Serialize};
use superstate_agent::state::State;
use superstate_provider::Message;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: u32,
    pub state: State,
    pub history: Vec<Message>,
}

/// Write atomically (a temporary file, then rename), so a crash mid-write never leaves a
/// truncated journal in place of the last good one.
pub fn write(path: &Path, state: &State, history: &[Message]) -> Result<(), String> {
    let body = serde_json::to_vec(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "state": state,
        "history": history,
    }))
    .map_err(|e| format!("journal: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("journal {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("journal {}: {e}", path.display()))
}

pub fn read(path: &Path) -> Result<Journal, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("journal {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("journal {}: {e}", path.display()))
}
```

  Add `pub mod journal;` to `crates/superstate-cli/src/lib.rs` (keep the list alphabetical). If `serde` or `superstate-provider` is not already a dependency of `superstate-cli`, add it to its `Cargo.toml` with the same version spec the workspace uses elsewhere.

- [ ] **Step 4: Flags.** In `crates/superstate-cli/src/cli.rs`, after the `fold_notes` field, add:

```rust
    /// Stop the run once it has cost this many US dollars at list price (needs a priced model)
    #[arg(long)]
    pub max_cost_usd: Option<f64>,

    /// Save the run before every model call to this file, so it can be resumed
    #[arg(long)]
    pub journal: Option<PathBuf>,

    /// Resume a run from a journal written by --journal (a fresh budget; the prompt is optional)
    #[arg(long, conflicts_with = "tdd")]
    pub resume: Option<PathBuf>,
```

  (Use the `PathBuf` import already in `cli.rs`; add `use std::path::PathBuf;` if absent.)

- [ ] **Step 5: Task runner.** In `crates/superstate-cli/src/task_runner.rs`:
  1. In `TaskConfig`, after `fold_notes`, add:

```rust
    /// Dollar limit (`--max-cost-usd`).
    pub max_cost_usd: Option<f64>,
    /// Where to save the run before every model call (`--journal`).
    pub journal: Option<PathBuf>,
    /// A journal to resume from (`--resume`).
    pub resume: Option<PathBuf>,
```

     In `From<&Cli>`, add `max_cost_usd: cli.max_cost_usd, journal: cli.journal.clone(), resume: cli.resume.clone(),`.
  2. In `run_task_with_state_and_writer`, directly before `state.orchestrator.review = config.review;`, add:

```rust
    // A resumed run takes its goal, gate, ledger and fold notes from the journal.
    let resumed = match &config.resume {
        Some(path) => {
            let j = crate::journal::read(path)?;
            *state = j.state;
            Some(j.history)
        }
        None => None,
    };
```

  3. In the `Budget { ... }` literal, change `max_cost_usd: None,` to `max_cost_usd: config.max_cost_usd,`.
  4. Replace the `on_event` closure with:

```rust
    let on_event = |event: superstate_agent::run::Event<'_>| {
        if let superstate_agent::run::Event::Checkpoint { history, state } = &event {
            if let Some(path) = &config.journal {
                if let Err(e) = crate::journal::write(path, state, history) {
                    eprintln!("{e}");
                }
            }
            return;
        }
        if let Ok(mut w) = writer_mutex.lock() {
            let _ = writeln!(w, "{}", crate::repl::format_event(&event));
        }
    };
```

  5. Replace

```rust
        None => {
            let (o, r) = runner.run(state);
            (o, r, Vec::new())
        }
```

     with

```rust
        None => {
            let (o, r) = match resumed {
                Some(history) => runner.resume(state, history),
                None => runner.run(state),
            };
            (o, r, Vec::new())
        }
```

- [ ] **Step 6: Report cost.** In `crates/superstate-cli/src/report.rs`:
  - In `RunRecord`, after `fold_note_failures`, add: `/// List-price cost in USD, when the model's price is known.` and `pub cost_usd: Option<f64>,`.
  - In `base`, add `cost_usd: None,`.
  - In `from_run`, add `cost_usd: superstate_provider::pricing(model_id).map(|p| p.cost(&report.usage)),`.

- [ ] **Step 7: `main.rs`: resume without a prompt.** In the `None =>` arm of `main`, replace

```rust
            if let Some(ref prompt) = cli.prompt {
                if prompt.trim().is_empty() {
```

  with

```rust
            if cli.prompt.is_some() || cli.resume.is_some() {
                if cli.resume.is_none() && cli.prompt.as_deref().unwrap_or("").trim().is_empty() {
```

  Leave the rest of the block unchanged.

- [ ] **Step 8: `TaskConfig` literals (mechanical).** In `crates/superstate-cli/src/repl.rs` (1 literal), `crates/superstate-cli/tests/test_task_runner.rs` (4 literals), `test_run_report.rs` (1) and `test_tdd_phases.rs` (1), add `max_cost_usd: None,`, `journal: None,` and `resume: None,` after `fold_notes: false,` in every `TaskConfig { ... }` literal.

- [ ] **Step 9: Run the new tests.** Run: `cargo test -p superstate --test test_journal --test test_run_report --test test_task_runner`. Expected: all pass (3 new).

- [ ] **Step 10: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 11: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(cli): --max-cost-usd, --journal and --resume; cost in the run report"
```
