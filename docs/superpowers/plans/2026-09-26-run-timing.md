# Run Telemetry: Where the Time Goes — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** The run report says where a run's wall time went: model time and tool time per turn, time per gate attempt, and how many times each tool was called.

**Why:** shadow run S3 took longer than agy with half the turns, and finding out why meant timing the test suite by hand (one test slept 122 s, inside every gate run). The report counts turns and tokens but not time, so "slower" cannot be explained from the report. Every speed change to blyte is judged on wall time; the report should show it.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass. Two existing tests build these structs field by field (`crates/blyte-cli/tests/test_run_report.rs` builds a full `RunReport`; `crates/blyte-cli/tests/test_fold_budget.rs` builds a `TurnUsage`). The only change allowed to them is adding `..Default::default()` to those struct literals so they compile with the new fields.
- No new dependencies. Timing uses `std::time::Instant`.
- New `TurnUsage` fields carry `#[serde(default)]`, so reports written before this change still deserialise.
- The report schema change is additive: `SCHEMA_VERSION` stays 1.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Time the run

**What to add** (names are part of the contract; the acceptance tests below check them):

In `crates/blyte-agent/src/run.rs`:
- `TurnUsage.model_ms: u64`: wall time of this turn's model call, including any provider retries and their waits.
- `TurnUsage.tools_ms: u64`: wall time spent executing this turn's tool calls (0 for a turn with none).
- `RunReport.gate_ms: Vec<u64>`: wall time of each gate attempt, in order (one entry per attempt; `gate_attempts` equals its length).
- `RunReport.tool_counts: BTreeMap<String, u32>`: calls per tool name (e.g. `{"bash": 3, "read": 5}`).

In `crates/blyte-cli/src/report.rs` (`RunRecord`, filled by `from_run`; zero or empty in `from_error`):
- `gate_ms: Vec<u64>` and `tool_counts: BTreeMap<String, u32>`, copied from the run report.
- `model_seconds: f64`, `tools_seconds: f64`, `gate_seconds: f64`: the sums, in seconds.

Where the timing goes is up to you; measure around the provider call (the whole retry loop), around each turn's tool executions, and around `run_gate`.

- [ ] **Step 1: Write the failing tests.**

Create `crates/blyte-agent/tests/test_run_timing.rs`:

```rust
//! The report says where the time went: model and tool time per turn, time per gate attempt,
//! calls per tool. Shadow run S3 was slower with half the turns and the report could not say why.

use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

/// Sleeps 100 ms per call; the first two calls ask for tools, then it stops.
struct Slow {
    calls: Mutex<u32>,
}

impl Provider for Slow {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        std::thread::sleep(Duration::from_millis(100));
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: format!("c{n}"),
            name: name.into(),
            arguments: args,
            signature: None,
        };
        Ok(match *n {
            1 => Response {
                tool_calls: vec![call("bash", json!({"command": "sleep 0.3"}))],
                ..Default::default()
            },
            2 => Response {
                tool_calls: vec![
                    call("read", json!({"path": "marker"})),
                    call("bash", json!({"command": "true"})),
                ],
                ..Default::default()
            },
            _ => Response::default(),
        })
    }
    fn model_id(&self) -> String {
        "slow".into()
    }
}

fn run(gate: &str) -> (Outcome, blyte_agent::run::RunReport) {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name c && \
             echo 0 > marker && git add -A && git commit -qm base",
            d.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    let p = Slow {
        calls: Mutex::new(0),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    r.run(&mut State::new("g", gate))
}

#[test]
fn every_turn_records_its_model_time() {
    let (_, rep) = run("true");
    assert!(!rep.turn_usage.is_empty());
    for t in &rep.turn_usage {
        assert!(t.model_ms >= 90, "turn {} model_ms {}", t.turn, t.model_ms);
    }
}

#[test]
fn tool_time_is_recorded_on_the_turn_that_ran_the_tools() {
    let (_, rep) = run("true");
    assert!(rep.turn_usage[0].tools_ms >= 250, "{}", rep.turn_usage[0].tools_ms);
    let last = rep.turn_usage.last().unwrap();
    assert_eq!(last.tools_ms, 0, "a turn with no tool calls spends no tool time");
}

#[test]
fn each_gate_attempt_is_timed() {
    let (outcome, rep) = run("sleep 0.3");
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_ms.len() as u32, rep.gate_attempts);
    assert!(rep.gate_ms[0] >= 250, "{:?}", rep.gate_ms);
}

#[test]
fn calls_are_counted_per_tool() {
    let (_, rep) = run("true");
    assert_eq!(rep.tool_counts.get("bash"), Some(&2));
    assert_eq!(rep.tool_counts.get("read"), Some(&1));
    assert_eq!(rep.tool_counts.values().sum::<u32>(), rep.tool_calls);
}
```

Create `crates/blyte-cli/tests/test_report_timing.rs`:

```rust
//! The run report carries the timing and per-tool counts, with totals in seconds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport, TurnUsage};
use blyte_agent::state::State;
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "p".into(),
        gate: Some("true".into()),
        context_strategy: "folded".into(),
        model: None,
        container: None,
        max_turns: 10,
        max_gates: 5,
        rollback: false,
        root: PathBuf::from("."),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        fold_budget: None,
    }
}

#[test]
fn the_record_carries_timing_and_tool_counts() {
    let report = RunReport {
        turn_usage: vec![
            TurnUsage { turn: 1, model_ms: 1500, tools_ms: 250, ..Default::default() },
            TurnUsage { turn: 2, model_ms: 500, tools_ms: 0, ..Default::default() },
        ],
        gate_attempts: 2,
        gate_ms: vec![3000, 4500],
        tool_counts: BTreeMap::from([("bash".to_string(), 2), ("read".to_string(), 1)]),
        ..Default::default()
    };
    let r = RunRecord::from_run(
        &config(),
        "scripted",
        &Outcome::Verified,
        &report,
        &State::new("p", "true"),
        Duration::from_secs(10),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["gate_ms"], serde_json::json!([3000, 4500]));
    assert_eq!(v["tool_counts"]["bash"], 2);
    assert_eq!(v["model_seconds"], 2.0);
    assert_eq!(v["tools_seconds"], 0.25);
    assert_eq!(v["gate_seconds"], 7.5);
    assert_eq!(v["turn_usage"][0]["model_ms"], 1500);
}

#[test]
fn an_error_record_has_zero_timing() {
    let v = serde_json::to_value(RunRecord::from_error(&config(), "e", Duration::ZERO)).unwrap();
    assert_eq!(v["gate_ms"], serde_json::json!([]));
    assert_eq!(v["model_seconds"], 0.0);
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_run_timing` and `cargo test -p blyte --test test_report_timing`. Expected: compile errors (the fields do not exist).

- [ ] **Step 3: Implement** the fields and timing above, and the `..Default::default()` edits to the two existing test literals.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_run_timing --test test_run` and `cargo test -p blyte --test test_report_timing --test test_run_report --test test_fold_budget`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(report): model, tool and gate time per run, and calls per tool"
```
