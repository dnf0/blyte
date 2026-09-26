# Record Why the Gate Failed — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** A run's report keeps what each failed gate attempt said, and a multi-step gate's summary
line names the steps that failed.

**Why (shadow run E5, 2026-09-26):** the run log showed only "Gate failed (attempt 1): 1 of 2 steps
failed". What failed, and why, was visible only to the model, so a 50-minute detour that followed it
could not be diagnosed afterwards. The report should keep the evidence.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified. The multi-step summary keeps its current start ("N of M steps
  failed"), so tests that look for it still pass.
- The report change is additive (`SCHEMA_VERSION` stays 1).
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Keep failed gate output in the report

**What to add** (names are part of the contract):
- `crates/blyte-agent/src/run.rs`:
  - The multi-step failure summary line becomes `"{n} of {m} steps failed: {steps}"`, where
    `{steps}` is the failing step commands joined with `", "`. The rest of the message is unchanged.
  - `RunReport.gate_failures: Vec<String>`: for each failed gate attempt, in order, the gate's output
    cut to its first 4,000 characters (append `"…"` when cut). Passing attempts add nothing.
- `crates/blyte-cli/src/report.rs`: `RunRecord.gate_failures: Vec<String>`, copied from the run
  report in `from_run`, empty in `from_error`.

- [ ] **Step 1: Write the failing tests.**

Create `crates/blyte-agent/tests/test_gate_failures.rs`:

```rust
//! The report keeps what each failed gate attempt said, and a multi-step summary names the failing
//! steps: a run's detour after "1 of 2 steps failed" could not be diagnosed from its log.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, RunReport, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper(Mutex<u32>);

impl Provider for Stopper {
    fn complete(&self, _: &Request) -> Result<Response, ProviderError> {
        *self.0.lock().unwrap() += 1;
        Ok(Response::default())
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

fn run(gate: &str, attempts: u32) -> RunReport {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             touch x && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    let p = Stopper(Mutex::new(0));
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget { max_turns: 6, max_gate_attempts: attempts, max_duration: None },
        rollback: false,
        on_event: None,
    };
    r.run(&mut State::new("g", gate)).1
}

#[test]
fn each_failed_attempt_is_kept() {
    let rep = run("echo boom-marker && exit 1", 2);
    assert_eq!(rep.gate_failures.len(), 2);
    assert!(rep.gate_failures[0].contains("boom-marker"), "{:?}", rep.gate_failures);
}

#[test]
fn the_multi_step_summary_names_the_failing_steps() {
    let rep = run("true\necho boom-marker && exit 1", 1);
    let first = rep.gate_failures[0].lines().next().unwrap();
    assert_eq!(first, "1 of 2 steps failed: echo boom-marker && exit 1");
}

#[test]
fn long_output_is_cut() {
    let rep = run("printf 'x%.0s' $(seq 1 20000); exit 1", 1);
    assert!(rep.gate_failures[0].chars().count() <= 4_001, "{}", rep.gate_failures[0].len());
    assert!(rep.gate_failures[0].ends_with('…'));
}

#[test]
fn a_passing_gate_adds_nothing() {
    assert!(run("true", 1).gate_failures.is_empty());
}
```

Create `crates/blyte-cli/tests/test_report_gate_failures.rs`:

```rust
//! The run record carries the failed gate output.

use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport};
use blyte_agent::state::State;
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "p".into(), gate: Some("false".into()), context_strategy: "folded".into(),
        model: None, container: None, max_turns: 5, max_gates: 2, rollback: false,
        root: PathBuf::from("."), container_workdir: "/workspace".into(), max_seconds: None,
        report: None, fold_budget: None,
    }
}

#[test]
fn the_record_carries_gate_failures() {
    let report = RunReport { gate_failures: vec!["exit 1\nboom".into()], ..Default::default() };
    let r = RunRecord::from_run(&config(), "m", &Outcome::GateFailed, &report,
                                &State::new("p", "false"), Duration::ZERO);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["gate_failures"][0], "exit 1\nboom");
    let e = serde_json::to_value(RunRecord::from_error(&config(), "e", Duration::ZERO)).unwrap();
    assert_eq!(e["gate_failures"], serde_json::json!([]));
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_gate_failures` and `cargo test -p blyte --test test_report_gate_failures`. Expected: compile errors.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_gate_failures --test test_gate_steps --test test_run` and `cargo test -p blyte --test test_report_gate_failures --test test_run_report`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(report): keep each failed gate attempt's output; name the failing steps"
```
