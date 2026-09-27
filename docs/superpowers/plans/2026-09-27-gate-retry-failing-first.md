# Gate Retries Run the Failing Steps First — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** after a failed gate, the next attempt runs the steps that failed **first**. If any of
them still fails, the attempt ends there, without re-running the steps that passed last time
(often the full test suite). Only when every previously failing step passes does the rest run.

**Why (2026-09-27):** a gate of `fmt` / `clippy` / `test` that failed on `clippy` reruns the whole
test suite on the next attempt even when clippy still fails, minutes of wall time the model then
waits through for no new information.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified** (in particular `test_gate_steps.rs` and
  `test_gate_failures.rs`: the failure message keeps the form `<n> of <total> steps failed: <steps>`).
- The first attempt of every gate behaves exactly as today (every step runs).
- A single-step gate behaves exactly as today.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Failing steps first on a retry

**Files:** `crates/blyte-agent/src/run.rs`, `crates/blyte-agent/tests/test_gate_retry_order.rs` (new).

**What to build:**

- `pub fn retry_order<'a>(steps: &[&'a str], last_failed: &[String]) -> (Vec<&'a str>, Vec<&'a str>)`
  in `run.rs`: splits `steps` into (the steps whose text is in `last_failed`, in gate order; the
  others, in gate order). With an empty `last_failed`, returns (all steps, nothing).
- The loop keeps the list of steps that failed on the previous gate attempt (empty before the
  first). `run_gate` takes it:
  - First attempt (or empty list): as today.
  - Later attempts: run the first group. If any of those fails, stop and report
    `<n> of <total> steps failed: <failed steps>` exactly as today, followed by one extra line:
    `Not re-run this attempt: <k> step(s) that passed last time (<steps>).` (omit the line when
    `k` is 0). If all of them pass, run the second group; the message then covers every failure
    of this attempt, as today.
  - Update the list of failing steps from this attempt's failures.
- `report.gate_ms` still has one entry per attempt (the whole attempt's time).

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_gate_retry_order.rs`:

```rust
//! A retry runs the previously failing gate steps first and stops there if they still fail:
//! re-running a passing full test suite while clippy still fails costs minutes and says nothing
//! new (2026-09-27).

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{retry_order, Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    seen: Mutex<Vec<Request>>,
}

impl Provider for Stopper {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response { text: "done".into(), ..Default::default() })
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name c && \
             touch base && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    d
}

fn run(d: &tempfile::TempDir, gate: &str, attempts: u32) -> (Outcome, String) {
    let p = Stopper { seen: Mutex::new(Vec::new()) };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget { max_turns: 8, max_gate_attempts: attempts, max_duration: None },
        rollback: false,
        on_event: None,
    };
    let mut st = State::new("g", gate);
    let (outcome, _) = r.run(&mut st);
    let last = format!("{:?}", p.seen.lock().unwrap().last().unwrap().messages);
    (outcome, last)
}

#[test]
fn retry_order_puts_the_failed_steps_first() {
    let steps = ["fmt", "clippy", "test"];
    assert_eq!(retry_order(&steps, &[]), (vec!["fmt", "clippy", "test"], vec![]));
    assert_eq!(retry_order(&steps, &["clippy".to_string()]), (vec!["clippy"], vec!["fmt", "test"]));
}

#[test]
fn a_step_that_passed_is_not_rerun_while_the_failing_one_still_fails() {
    let d = repo();
    // Step 1 always fails; step 2 passes and records every run.
    let (outcome, last) = run(&d, "echo still-broken && exit 1\necho ran >> slow.log", 3);
    assert_eq!(outcome, Outcome::GateFailed);
    let runs = std::fs::read_to_string(d.path().join("slow.log")).unwrap();
    assert_eq!(runs.lines().count(), 1, "the passing step ran on the first attempt only");
    assert!(last.contains("1 of 2 steps failed"), "{last:.600}");
    assert!(last.contains("Not re-run this attempt: 1 step(s) that passed last time"), "{last:.600}");
}

#[test]
fn once_the_failing_step_passes_the_rest_runs_again() {
    let d = repo();
    // Step 1 fails on the first attempt only (it creates its own fix); step 2 records each run.
    let gate = "test -f fixed || (touch fixed && exit 1)\necho ran >> slow.log";
    let (outcome, _) = run(&d, gate, 3);
    assert_eq!(outcome, Outcome::Verified);
    let runs = std::fs::read_to_string(d.path().join("slow.log")).unwrap();
    assert_eq!(runs.lines().count(), 2, "first attempt, then again once step 1 passed");
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_gate_retry_order`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): a gate retry runs the failing steps first and stops if they still fail"
```
