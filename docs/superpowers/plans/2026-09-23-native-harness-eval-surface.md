# Native Harness Eval Surface Implementation Plan (Part A: superstate)

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Make the native harness measurable by `superstate-eval`: an ablatable loop-breaker that cannot starve the worker, a self-enforced deadline, agy-compatible model effort suffixes, and a `--report` JSON contract.

**Spec:** `docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md`

**Architecture:** Three independent layers. `superstate-agent` gains `Runner.loop_breaker`, an insist escape and `Budget.max_duration`. `superstate-provider`'s Gemini adapter maps `-low/-medium/-high` onto `thinkingLevel`. `superstate-cli` exposes flags and writes a `RunRecord` JSON report from `task_runner`.

**Tech Stack:** Rust 2021, `clap` derive, `serde`/`serde_json`, `tempfile`.

## Global Constraints

- Base branch: `feat/native-harness-eval-surface`. Each task runs in its own worktree `.wt/<task>` on branch `wip/<task>`, created from the base branch. Claude merges reviewed work back.
- Task 1 and Task 2 are independent and may run in parallel. Task 3 requires Task 1: it sets `Runner.loop_breaker` and `Budget.max_duration`.
- Zero warnings: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass before every commit.
- Conventional commits with `--no-gpg-sign`, using the exact messages below.
- Do not change behaviour outside the task: existing tests must pass unmodified, except for adding the new struct fields to literals where a step says so.

---

### Task 1: Loop-breaker switch, insist escape, and run deadline (`superstate-agent`)

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Modify (add fields to struct literals only): `crates/superstate-agent/examples/solve.rs`, `crates/superstate-agent/tests/test_run.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/tests/test_action_ledger_e2e.rs`, `crates/superstate-cli/tests/test_working_set_e2e.rs`
- Test: `crates/superstate-cli/tests/test_working_set_e2e.rs` (append)

- [ ] **Step 1: Write the failing tests.** Append to the end of `crates/superstate-cli/tests/test_working_set_e2e.rs`:

```rust
#[test]
fn test_loop_breaker_serves_an_insisted_read_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");
    std::fs::write(
        repo.path().join("greet.py"),
        "def greet():\n    return 'hello'\n",
    )
    .unwrap();

    // Turn 2 is intercepted. Turn 3 repeats it verbatim: insisting wins, so it is served
    // rather than refused again (FINDINGS 13.22 / 13.25).
    let provider = ScriptedProvider::new(vec![
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        says("Done reading."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &StateOnly,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    let summaries: Vec<&str> = state
        .runtime
        .actions
        .iter()
        .map(|a| a.summary.as_str())
        .collect();
    assert_eq!(
        summaries,
        vec![
            "2 lines read",
            "intercepted: already in working set",
            "2 lines read"
        ]
    );
}

#[test]
fn test_loop_breaker_off_executes_every_read_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");
    std::fs::write(
        repo.path().join("greet.py"),
        "def greet():\n    return 'hello'\n",
    )
    .unwrap();

    let provider = ScriptedProvider::new(vec![
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        says("Done reading."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &StateOnly,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    let summaries: Vec<&str> = state
        .runtime
        .actions
        .iter()
        .map(|a| a.summary.as_str())
        .collect();
    assert_eq!(summaries, vec!["2 lines read", "2 lines read"]);
}

#[test]
fn test_budget_max_duration_ends_the_run_before_the_next_turn() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");

    let provider = ScriptedProvider::new(vec![calls("read", json!({"path": "greet.py"}))]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &StateOnly,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: Some(std::time::Duration::ZERO),
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(report.outcome_turns, 0);
    assert!(
        provider.seen.lock().unwrap().is_empty(),
        "no model call may start once the deadline has passed"
    );
}
```

- [ ] **Step 2: Run to confirm they fail to compile.** Run: `cargo test -p superstate-cli --test test_working_set_e2e`. Expected: compile errors about `max_duration` not being a field of `Budget` and `loop_breaker` not being a field of `Runner`.

- [ ] **Step 3: Add `Budget.max_duration`.** In `crates/superstate-agent/src/run.rs`, replace the `Budget` struct and its `Default` impl with:

```rust
#[derive(Debug, Clone)]
pub struct Budget {
    pub max_turns: u32,
    pub max_gate_attempts: u32,
    /// Wall-clock limit, checked before each turn. The harness stops itself and reports, rather
    /// than being killed from outside with its telemetry lost (FINDINGS 13.5).
    pub max_duration: Option<Duration>,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_turns: 60,
            max_gate_attempts: 5,
            max_duration: None,
        }
    }
}
```

- [ ] **Step 4: Add `Runner.loop_breaker`.** In the `Runner` struct in `run.rs`, insert this field directly after `pub rollback: bool,`:

```rust
    /// Run the loop-breaker guards (redundant read and inspection interception). Default on;
    /// off is the ablation control, because a guard that changes what the worker sees must be
    /// measured, not assumed (FINDINGS 13.25).
    pub loop_breaker: bool,
```

- [ ] **Step 5: Add the insist check.** In `run.rs`, add this function directly above `pub fn should_intercept_redundant_read(`:

```rust
/// True when `call` repeats, verbatim, a call the loop-breaker intercepted on the immediately
/// preceding action. Insisting wins: a worker that re-issues a refused request has decided it
/// needs it, and refusing again only spends the turn budget (FINDINGS 13.22, 13.25).
pub fn is_insist(call: &ToolCall, actions: &[ActionEntry]) -> bool {
    actions.last().is_some_and(|a| {
        a.summary.starts_with("intercepted")
            && a.tool == call.name
            && a.target == extract_action_target(&call.name, &call.arguments)
    })
}
```

- [ ] **Step 6: Gate the guards.** In `Runner::run`, find the block that starts with `let outcome = if let Some(guard_msg) = should_intercept_redundant_read(`. Directly above the comment `// Intercept redundant file reads and redundant read-only inspections before execution`, insert:

```rust
                    let guards_on =
                        self.loop_breaker && !is_insist(call, &state.runtime.actions);
```

Then change the two guard conditions so each only runs when `guards_on` is true. The first becomes:

```rust
                    let outcome = if let Some(guard_msg) = guards_on
                        .then(|| {
                            should_intercept_redundant_read(
                                call,
                                &state.runtime.working_set,
                                &state.runtime.actions,
                            )
                        })
                        .flatten()
                    {
```

The second becomes:

```rust
                    } else if let Some(guard_msg) = guards_on
                        .then(|| {
                            should_intercept_redundant_bash(
                                call,
                                &state.runtime.actions,
                                &state.runtime.working_set,
                                &state.orchestrator.verification_cmd,
                            )
                        })
                        .flatten()
                    {
```

Leave the bodies of both branches, and every other branch, unchanged.

- [ ] **Step 7: Enforce the deadline.** In `Runner::run`, directly above the line `for turn in 1..=self.budget.max_turns {`, insert:

```rust
        let started = std::time::Instant::now();
```

As the first statements inside that `for` loop, before `report.outcome_turns = turn;`, insert:

```rust
            if let Some(limit) = self.budget.max_duration {
                if started.elapsed() >= limit {
                    return (Outcome::BudgetExhausted, report);
                }
            }
```

- [ ] **Step 8: Update every struct literal.** Add `loop_breaker: true,` directly after the `rollback: …,` line of every `Runner { … }` literal. Add `max_duration: None,` as the last field of every `Budget { … }` literal. Do not change any other field. The known sites are below; `cargo check --workspace --all-targets` lists any missed one.
  - `Runner`: `examples/solve.rs:84`, `tests/test_run.rs:93` and `:321` (superstate-agent); `src/task_runner.rs:167`, `tests/test_action_ledger_e2e.rs:194` and `:265`, `tests/test_working_set_e2e.rs:205`, `:310` and `:367` (superstate-cli). In `test_run.rs:93` the field may be set from a helper parameter; use `loop_breaker: true,` regardless.
  - `Budget`: the `budget: Budget {` / `budget: superstate_agent::run::Budget {` literals at the same sites.

- [ ] **Step 9: Run the new tests.** Run: `cargo test -p superstate-cli --test test_working_set_e2e`. Expected: all tests pass, including the three new ones.

- [ ] **Step 10: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: no formatting diff, no warnings, 0 failed tests.

- [ ] **Step 11: Commit.**

```bash
git add -A crates/
git commit --no-gpg-sign -m "feat(agent): ablatable loop-breaker with insist escape and run deadline"
```

---

### Task 2: Effort suffix to Gemini `thinkingLevel` (`superstate-provider`)

**Files:**
- Modify: `crates/superstate-provider/src/gemini.rs`
- Test: `crates/superstate-provider/tests/test_gemini_wire.rs` (append)

- [ ] **Step 1: Write the failing tests.** Append to `crates/superstate-provider/tests/test_gemini_wire.rs`:

```rust
#[test]
fn effort_suffix_becomes_thinking_level_and_leaves_the_model_id() {
    use superstate_provider::Provider;
    let g = gemini().with_model("gemini-3.8-flash-high");
    assert_eq!(g.model_id(), "gemini/gemini-3.8-flash");
    assert_eq!(g.thinking_level(), Some("high"));
    let b = g.body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert_eq!(
        b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "high"
    );
}

#[test]
fn a_bare_model_sends_no_generation_config() {
    let g = gemini().with_model("gemini-3.8-flash");
    assert_eq!(g.thinking_level(), None);
    let b = g.body(&Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    });
    assert!(b.get("generationConfig").is_none());
}

#[test]
fn unsupported_effort_suffixes_stay_in_the_model_id() {
    // -xhigh and -max have no Gemini equivalent. They must reach the API and fail loudly,
    // never run at a silently different effort.
    use superstate_provider::Provider;
    for m in ["gemini-3.8-flash-xhigh", "gemini-3.8-flash-max"] {
        let g = gemini().with_model(m);
        assert_eq!(g.model_id(), format!("gemini/{m}"));
        assert_eq!(g.thinking_level(), None);
    }
}

#[test]
fn thinking_level_and_max_output_tokens_coexist() {
    let g = gemini().with_model("gemini-3.8-flash-low");
    let b = g.body(&Request {
        messages: vec![Message::User("hi".into())],
        max_output_tokens: Some(1024),
        ..Default::default()
    });
    assert_eq!(b["generationConfig"]["maxOutputTokens"], 1024);
    assert_eq!(b["generationConfig"]["thinkingConfig"]["thinkingLevel"], "low");
}

#[test]
fn split_effort_handles_each_supported_level() {
    use superstate_provider::gemini::split_effort;
    assert_eq!(
        split_effort("gemini-3.8-flash-low"),
        ("gemini-3.8-flash".to_string(), Some("low".to_string()))
    );
    assert_eq!(
        split_effort("gemini-3.8-flash-medium"),
        ("gemini-3.8-flash".to_string(), Some("medium".to_string()))
    );
    assert_eq!(
        split_effort("gemini-3.8-flash-high"),
        ("gemini-3.8-flash".to_string(), Some("high".to_string()))
    );
    assert_eq!(
        split_effort("gemini-3.8-flash"),
        ("gemini-3.8-flash".to_string(), None)
    );
    assert_eq!(split_effort("-high"), ("-high".to_string(), None));
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-provider --test test_gemini_wire`. Expected: compile errors, since `thinking_level` and `split_effort` do not exist.

- [ ] **Step 3: Add `split_effort`.** In `crates/superstate-provider/src/gemini.rs`, directly above `pub struct Gemini {`, insert:

```rust
/// agy's effort suffixes that map onto Gemini's `thinkingLevel`. `-xhigh` and `-max` are
/// deliberately absent: they have no Gemini equivalent, so they stay in the model id and the
/// API rejects the request instead of running at a silently different effort.
const EFFORT_LEVELS: [&str; 3] = ["low", "medium", "high"];

/// Split `gemini-3.8-flash-high` into `("gemini-3.8-flash", Some("high"))`.
///
/// The eval pins every driver to one model string, agy's, which carries the effort as a
/// suffix. Sending that string to the API verbatim either fails or silently drops the effort --
/// the "`--model` not pinned" parity defect in another form.
pub fn split_effort(model: &str) -> (String, Option<String>) {
    for level in EFFORT_LEVELS {
        if let Some(base) = model.strip_suffix(&format!("-{level}")) {
            if !base.is_empty() {
                return (base.to_string(), Some(level.to_string()));
            }
        }
    }
    (model.to_string(), None)
}
```

- [ ] **Step 4: Carry the level on the provider.** Replace the `Gemini` struct, `from_env` and `with_model` with:

```rust
pub struct Gemini {
    api_key: String,
    model: String,
    thinking_level: Option<String>,
}

impl Gemini {
    /// Reads `GEMINI_API_KEY`, falling back to `GOOGLE_API_KEY` as pillars does.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .map_err(|_| ProviderError::MissingKey("GEMINI_API_KEY"))?;
        let (model, thinking_level) = split_effort(
            &std::env::var("SUPERSTATE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
        );
        Ok(Self {
            api_key,
            model,
            thinking_level,
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let (model, thinking_level) = split_effort(&model.into());
        self.model = model;
        self.thinking_level = thinking_level;
        self
    }

    /// The `thinkingLevel` sent with every request, if the model string carried an effort.
    pub fn thinking_level(&self) -> Option<&str> {
        self.thinking_level.as_deref()
    }
```

Keep the rest of the `impl Gemini` block (starting at `pub fn body`) unchanged. If this replacement closes the `impl` block early, remove the duplicate brace so the block stays one `impl`.

- [ ] **Step 5: Send it.** In `Gemini::body`, replace:

```rust
        if let Some(max) = req.max_output_tokens {
            body["generationConfig"] = json!({"maxOutputTokens": max});
        }
```

with:

```rust
        let mut generation = serde_json::Map::new();
        if let Some(max) = req.max_output_tokens {
            generation.insert("maxOutputTokens".into(), json!(max));
        }
        if let Some(level) = &self.thinking_level {
            generation.insert("thinkingConfig".into(), json!({"thinkingLevel": level}));
        }
        if !generation.is_empty() {
            body["generationConfig"] = Value::Object(generation);
        }
```

- [ ] **Step 6: Run the tests.** Run: `cargo test -p superstate-provider`. Expected: all pass, including the five new tests.

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [ ] **Step 8: Commit.**

```bash
git add crates/superstate-provider/
git commit --no-gpg-sign -m "feat(provider): map agy effort suffixes onto Gemini thinkingLevel"
```

---

### Task 3: `--report`, `--no-loop-breaker`, `--max-seconds`, `--container-workdir` (`superstate-cli`)

Requires Task 1 merged into the base branch.

**Files:**
- Create: `crates/superstate-cli/src/report.rs`
- Modify: `crates/superstate-cli/src/lib.rs`, `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/src/repl.rs` (TaskConfig literal only), `crates/superstate-cli/tests/test_task_runner.rs` (TaskConfig literals only)
- Test: `crates/superstate-cli/tests/test_run_report.rs`

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-cli/tests/test_run_report.rs`:

```rust
//! The `--report` JSON contract that superstate-eval reads. Schema:
//! docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use superstate_agent::run::{Outcome, RunReport};
use superstate_agent::state::{ActionEntry, State};
use superstate_cli::report::{count_insists, count_interceptions, RunRecord};
use superstate_cli::task_runner::TaskConfig;
use superstate_provider::Usage;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "fix it".into(),
        gate: None,
        context_strategy: "state-only".into(),
        model: Some("gemini-3.8-flash-high".into()),
        container: Some("abc123".into()),
        max_turns: 1000,
        max_gates: 5,
        rollback: true,
        root: PathBuf::from("."),
        loop_breaker: false,
        container_workdir: "/app".into(),
        max_seconds: Some(600),
        report: None,
    }
}

fn action(tool: &str, target: &str, summary: &str) -> ActionEntry {
    ActionEntry {
        turn: 1,
        tool: tool.into(),
        target: target.into(),
        summary: summary.into(),
        is_error: false,
    }
}

#[test]
fn interceptions_are_counted_by_kind() {
    let actions = vec![
        action("read", "a.py", "intercepted: already in working set"),
        action("read", "b.py", "intercepted: already in working set"),
        action("bash", "git status", "intercepted: redundant inspection"),
        action("read", "a.py", "2 lines read"),
    ];
    let i = count_interceptions(&actions);
    assert_eq!((i.read, i.bash), (2, 1));
}

#[test]
fn an_insist_is_a_served_repeat_of_the_intercepted_call() {
    let actions = vec![
        action("read", "a.py", "2 lines read"),
        action("read", "a.py", "intercepted: already in working set"),
        action("read", "a.py", "2 lines read"),
        action("bash", "git status", "intercepted: redundant inspection"),
        action("bash", "git diff", "exit 0"),
        action("read", "b.py", "intercepted: already in working set"),
        action("read", "b.py", "intercepted: already in working set"),
    ];
    assert_eq!(count_insists(&actions), 1);
}

#[test]
fn an_error_record_has_zeroed_counters_and_no_model_id() {
    let r = RunRecord::from_error(
        &config(),
        "Provider error: no API key",
        Duration::from_millis(1500),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["error"], "Provider error: no API key");
    assert_eq!(v["model_requested"], "gemini-3.8-flash-high");
    assert!(v["model_id"].is_null());
    assert_eq!(v["context"], "state-only");
    assert_eq!(v["loop_breaker"], false);
    assert_eq!(v["gated"], false);
    assert_eq!(v["container"], "abc123");
    assert_eq!(v["container_workdir"], "/app");
    assert_eq!(v["duration_seconds"], 1.5);
    assert_eq!(v["turns"], 0);
    assert_eq!(v["empty_turns"], 0);
    assert_eq!(v["tool_calls"], 0);
    assert_eq!(v["gate_attempts"], 0);
    assert_eq!(v["rollbacks"], 0);
    assert_eq!(
        v["usage"],
        json!({"input": 0, "cached": 0, "output": 0, "thinking": 0})
    );
    assert_eq!(v["interceptions"], json!({"read": 0, "bash": 0}));
    assert_eq!(v["insists"], 0);
    assert_eq!(v["agent_note_keys"], json!([]));
}

#[test]
fn a_completed_run_maps_every_field() {
    let mut state = State::new("fix it", "");
    state
        .agent
        .fields
        .insert("next_step".into(), json!("run tests"));
    state
        .agent
        .fields
        .insert("hypothesis".into(), json!("off by one"));
    state.runtime.rollbacks = 2;
    state.runtime.actions = vec![
        action("read", "a.py", "2 lines read"),
        action("read", "a.py", "intercepted: already in working set"),
        action("read", "a.py", "2 lines read"),
    ];
    let report = RunReport {
        outcome_turns: 7,
        usage: Usage {
            input: 100,
            cached: 50,
            output: 20,
            thinking: 5,
        },
        empty_turns: 1,
        tool_calls: 3,
        gate_attempts: 2,
    };
    let r = RunRecord::from_run(
        &config(),
        "gemini/gemini-3.8-flash",
        &Outcome::BudgetExhausted,
        &report,
        &state,
        Duration::from_secs(3),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["outcome"], "budget_exhausted");
    assert!(v["error"].is_null());
    assert_eq!(v["model_id"], "gemini/gemini-3.8-flash");
    assert_eq!(v["turns"], 7);
    assert_eq!(v["empty_turns"], 1);
    assert_eq!(v["tool_calls"], 3);
    assert_eq!(v["gate_attempts"], 2);
    assert_eq!(v["rollbacks"], 2);
    assert_eq!(
        v["usage"],
        json!({"input": 100, "cached": 50, "output": 20, "thinking": 5})
    );
    assert_eq!(v["interceptions"], json!({"read": 1, "bash": 0}));
    assert_eq!(v["insists"], 1);
    assert_eq!(v["agent_note_keys"], json!(["hypothesis", "next_step"]));
}

#[test]
fn an_error_outcome_carries_its_message() {
    let state = State::new("fix it", "");
    let r = RunRecord::from_run(
        &config(),
        "gemini/gemini-3.8-flash",
        &Outcome::Error("provider returned 500".into()),
        &RunReport::default(),
        &state,
        Duration::from_secs(1),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["error"], "provider returned 500");
}

#[test]
fn gated_is_true_only_for_a_non_empty_gate() {
    let mut blank = config();
    blank.gate = Some("   ".into());
    assert!(!RunRecord::from_error(&blank, "x", Duration::ZERO).gated);
    let mut real = config();
    real.gate = Some("pytest".into());
    assert!(RunRecord::from_error(&real, "x", Duration::ZERO).gated);
}

#[test]
fn the_binary_writes_a_report_when_the_provider_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("run.json");
    let out = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "fix it",
            "--model",
            "gemini-3.8-flash-high",
            "--context",
            "state-only",
            "--no-loop-breaker",
            "--container-workdir",
            "/app",
            "--max-seconds",
            "600",
            "--report",
        ])
        .arg(&report)
        .current_dir(dir.path())
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report must exist"))
            .unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["outcome"], "error");
    assert!(v["error"].as_str().unwrap().contains("GEMINI_API_KEY"));
    assert_eq!(v["model_requested"], "gemini-3.8-flash-high");
    assert!(v["model_id"].is_null());
    assert_eq!(v["context"], "state-only");
    assert_eq!(v["loop_breaker"], false);
    assert_eq!(v["gated"], false);
    assert!(v["container"].is_null());
    assert_eq!(v["container_workdir"], "/app");
    assert_eq!(v["turns"], 0);
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-cli --test test_run_report`. Expected: compile errors, since `superstate_cli::report` does not exist and `TaskConfig` lacks the new fields.

- [ ] **Step 3: Create the report module.** Create `crates/superstate-cli/src/report.rs`:

```rust
//! The headless solver's machine-readable run report (`--report <path>`).
//!
//! This is the contract between the harness and anything that measures it: superstate-eval
//! reads exactly this shape. Every field is always written, because a zero is a measurement and
//! an absent key is a bug. Schema: docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use superstate_agent::run::{Outcome, RunReport};
use superstate_agent::state::{ActionEntry, State};

use crate::task_runner::TaskConfig;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct UsageRecord {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thinking: u64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Interceptions {
    pub read: u32,
    pub bash: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunRecord {
    pub schema_version: u32,
    pub outcome: String,
    pub error: Option<String>,
    pub model_requested: Option<String>,
    pub model_id: Option<String>,
    pub context: String,
    pub loop_breaker: bool,
    pub gated: bool,
    pub container: Option<String>,
    pub container_workdir: String,
    pub duration_seconds: f64,
    pub turns: u32,
    pub empty_turns: u32,
    pub tool_calls: u32,
    pub gate_attempts: u32,
    pub rollbacks: u32,
    pub usage: UsageRecord,
    pub interceptions: Interceptions,
    pub insists: u32,
    pub agent_note_keys: Vec<String>,
}

impl RunRecord {
    fn base(config: &TaskConfig, elapsed: Duration) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            outcome: "error".into(),
            error: None,
            model_requested: config.model.clone(),
            model_id: None,
            context: config.context_strategy.clone(),
            loop_breaker: config.loop_breaker,
            gated: config
                .gate
                .as_deref()
                .is_some_and(|g| !g.trim().is_empty()),
            container: config.container.clone(),
            container_workdir: config.container_workdir.clone(),
            duration_seconds: elapsed.as_secs_f64(),
            turns: 0,
            empty_turns: 0,
            tool_calls: 0,
            gate_attempts: 0,
            rollbacks: 0,
            usage: UsageRecord::default(),
            interceptions: Interceptions::default(),
            insists: 0,
            agent_note_keys: Vec::new(),
        }
    }

    /// A run that never reached the loop, e.g. a missing API key.
    pub fn from_error(config: &TaskConfig, error: &str, elapsed: Duration) -> Self {
        Self {
            error: Some(error.to_string()),
            ..Self::base(config, elapsed)
        }
    }

    /// A run the loop completed, whatever its outcome.
    pub fn from_run(
        config: &TaskConfig,
        model_id: &str,
        outcome: &Outcome,
        report: &RunReport,
        state: &State,
        elapsed: Duration,
    ) -> Self {
        let (outcome_name, error) = match outcome {
            Outcome::Verified => ("verified", None),
            Outcome::BudgetExhausted => ("budget_exhausted", None),
            Outcome::GateFailed => ("gate_failed", None),
            Outcome::Error(e) => ("error", Some(e.clone())),
        };
        let mut agent_note_keys: Vec<String> = state.agent.fields.keys().cloned().collect();
        agent_note_keys.sort();
        Self {
            outcome: outcome_name.into(),
            error,
            model_id: Some(model_id.to_string()),
            turns: report.outcome_turns,
            empty_turns: report.empty_turns,
            tool_calls: report.tool_calls,
            gate_attempts: report.gate_attempts,
            rollbacks: state.runtime.rollbacks,
            usage: UsageRecord {
                input: report.usage.input,
                cached: report.usage.cached,
                output: report.usage.output,
                thinking: report.usage.thinking,
            },
            interceptions: count_interceptions(&state.runtime.actions),
            insists: count_insists(&state.runtime.actions),
            agent_note_keys,
            ..Self::base(config, elapsed)
        }
    }
}

/// Loop-breaker interceptions, by guard.
pub fn count_interceptions(actions: &[ActionEntry]) -> Interceptions {
    let mut counts = Interceptions::default();
    for a in actions {
        match a.summary.as_str() {
            "intercepted: already in working set" => counts.read += 1,
            "intercepted: redundant inspection" => counts.bash += 1,
            _ => {}
        }
    }
    counts
}

/// Calls executed immediately after an interception of the same tool and target: the worker
/// insisted. EXP-081 found 90% of perimeter suppressions were insisted on; this is the number
/// that says whether the native guard fares any better.
pub fn count_insists(actions: &[ActionEntry]) -> u32 {
    actions
        .windows(2)
        .filter(|w| {
            w[0].summary.starts_with("intercepted")
                && !w[1].summary.starts_with("intercepted")
                && w[0].tool == w[1].tool
                && w[0].target == w[1].target
        })
        .count() as u32
}

pub fn write(path: &Path, record: &RunRecord) -> Result<(), String> {
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("report serialisation failed: {e}"))?;
    std::fs::write(path, json + "\n")
        .map_err(|e| format!("cannot write report {}: {e}", path.display()))
}
```

- [ ] **Step 4: Register the module.** In `crates/superstate-cli/src/lib.rs`, add `pub mod report;` between `pub mod repl;` and `pub mod state_cli;`.

- [ ] **Step 5: Add the flags.** In `crates/superstate-cli/src/cli.rs`, in `struct Cli`, insert directly after the `pub rollback: bool,` field (before `#[command(subcommand)]`):

```rust
    /// Disable the loop-breaker guards (redundant read / inspection interception). Ablation control
    #[arg(long)]
    pub no_loop_breaker: bool,

    /// Working directory inside the container when --container is set
    #[arg(long, default_value = "/workspace")]
    pub container_workdir: String,

    /// Wall-clock budget in seconds; the run stops itself and reports when it is spent
    #[arg(long)]
    pub max_seconds: Option<u64>,

    /// Write a JSON run report to this path when the headless run exits
    #[arg(long)]
    pub report: Option<PathBuf>,
```

- [ ] **Step 6: Extend `TaskConfig`.** In `crates/superstate-cli/src/task_runner.rs`, add these fields at the end of `struct TaskConfig` (after `pub root: PathBuf,`):

```rust
    /// Run the loop-breaker guards. False is the ablation control (`--no-loop-breaker`).
    pub loop_breaker: bool,
    /// Working directory inside the container (`--container-workdir`).
    pub container_workdir: String,
    /// Wall-clock budget in seconds (`--max-seconds`).
    pub max_seconds: Option<u64>,
    /// Where to write the JSON run report, if anywhere (`--report`).
    pub report: Option<PathBuf>,
```

In `impl From<&crate::cli::Cli> for TaskConfig`, add after `root,`:

```rust
            loop_breaker: !cli.no_loop_breaker,
            container_workdir: cli.container_workdir.clone(),
            max_seconds: cli.max_seconds,
            report: cli.report.clone(),
```

In every other `TaskConfig { … }` literal (`src/repl.rs:562`; `tests/test_task_runner.rs:73`, `:102`, `:120`, `:149`), add as the last fields:

```rust
                loop_breaker: true,
                container_workdir: "/workspace".into(),
                max_seconds: None,
                report: None,
```

(adjust indentation to match the literal).

- [ ] **Step 7: Write the report from the runner.** In `task_runner.rs`, replace the body of `pub fn run_task` with:

```rust
    let started = std::time::Instant::now();
    let verification_cmd = config.gate.as_deref().unwrap_or("");
    let mut state = superstate_agent::state::State::new(&config.prompt, verification_cmd);
    let mut stdout = std::io::stdout();
    let result = run_task_with_state_and_writer(&config, &mut state, &mut stdout);
    // The loop writes its own report; this covers a run that never reached it (e.g. no API
    // key), so a caller always finds one. An absent report would read as a crash.
    if let (Err(e), Some(path)) = (&result, &config.report) {
        crate::report::write(
            path,
            &crate::report::RunRecord::from_error(&config, e, started.elapsed()),
        )?;
    }
    result
```

In `run_task_with_state_and_writer`:
1. Insert `let started = std::time::Instant::now();` as the first statement of the function body.
2. In the `ContainerShell { … }` literal, replace `workdir: PathBuf::from("/workspace"),` with `workdir: PathBuf::from(&config.container_workdir),`.
3. In the `match (&container_shell, &container_fs)`, replace `(Some(cs), Some(cf)) => (cs, cf, PathBuf::from("/workspace")),` with `(Some(cs), Some(cf)) => (cs, cf, PathBuf::from(&config.container_workdir)),`.
4. In the `Budget` literal, replace `max_duration: None,` (added by Task 1) with `max_duration: config.max_seconds.map(std::time::Duration::from_secs),`.
5. In the `Runner` literal, replace `loop_breaker: true,` (added by Task 1) with `loop_breaker: config.loop_breaker,`.
6. Replace the final two lines:

```rust
    let (outcome, report) = runner.run(state);
    Ok((outcome, report))
```

with:

```rust
    let (outcome, report) = runner.run(state);
    if let Some(path) = &config.report {
        let model_id = superstate_provider::Provider::model_id(provider.as_ref());
        let record = crate::report::RunRecord::from_run(
            config,
            &model_id,
            &outcome,
            &report,
            state,
            started.elapsed(),
        );
        crate::report::write(path, &record)?;
    }
    Ok((outcome, report))
```

- [ ] **Step 8: Run the new tests.** Run: `cargo test -p superstate-cli --test test_run_report`. Expected: 7 passed.

- [ ] **Step 9: Check the help text.** Run: `cargo run -q --bin superstate -- --help`. Expected: `--no-loop-breaker`, `--container-workdir <CONTAINER_WORKDIR>` (default `/workspace`), `--max-seconds <MAX_SECONDS>` and `--report <REPORT>` are listed.

- [ ] **Step 10: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [ ] **Step 11: Commit.**

```bash
git add -A crates/superstate-cli/
git commit --no-gpg-sign -m "feat(cli): headless run report, deadline, loop-breaker and container workdir flags"
```
