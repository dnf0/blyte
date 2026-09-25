# Gate Steps: Report Every Failure at Once — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** A gate given as several lines is several steps. blyte runs every step, even after one fails, and reports all failing steps in one message. A single-line gate behaves exactly as before.

**Why (shadow run S1-read-budget, 2026-09-25):** the gate was `cargo fmt --all --check && cargo clippy ... && cargo test ...`. The first gate failed on fmt; the model fixed it and stopped; the second gate failed on clippy, which `&&` had hidden. That second round cost 7 of the run's 39 turns plus a full gate run. Running every step and reporting all failures turns two rounds into one.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Do not change the CLI flag's type: `--gate` stays one string (`Option<String>`); steps are its lines.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Multi-step gate

**Files:** `crates/blyte-agent/src/run.rs`; `crates/blyte-cli/src/cli.rs` (doc comment only); new test `crates/blyte-agent/tests/test_gate_steps.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_gate_steps.rs`:

```rust
//! A gate of several lines is several steps: every step runs and every failure is reported
//! together. With `fmt && clippy && test`, a shadow run found the clippy failure only after
//! fixing the fmt one -- a whole extra gate round of 7 turns.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

/// Always stops (no tool calls), so every turn triggers the gate. Records each request.
struct Stopper {
    seen: Mutex<Vec<Request>>,
}

impl Provider for Stopper {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
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

/// Runs with the given gate and at most two gate attempts; returns the outcome, gate attempts,
/// the system prompt and the text of the last request (which carries the gate feedback).
fn run(d: &tempfile::TempDir, gate: &str) -> (Outcome, u32, String, String) {
    let p = Stopper {
        seen: Mutex::new(Vec::new()),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    let mut st = State::new("g", gate);
    let (outcome, rep) = r.run(&mut st);
    let seen = p.seen.lock().unwrap();
    let sys = seen[0].system.clone();
    let last = format!("{:?}", seen.last().unwrap().messages);
    (outcome, rep.gate_attempts, sys, last)
}

#[test]
fn every_failing_step_is_reported_in_one_message() {
    let d = repo();
    let (outcome, _, _, last) = run(
        &d,
        "echo fmt-is-wrong && exit 1\ntrue\necho clippy-is-wrong && exit 2",
    );
    assert_eq!(outcome, Outcome::GateFailed);
    assert!(last.contains("2 of 3 steps failed"), "{last:.600}");
    assert!(last.contains("fmt-is-wrong"), "{last:.600}");
    assert!(last.contains("clippy-is-wrong"), "{last:.600}");
}

#[test]
fn a_step_runs_even_after_an_earlier_one_fails() {
    let d = repo();
    run(&d, "false\ntouch later-step-ran");
    assert!(d.path().join("later-step-ran").exists());
}

#[test]
fn a_gate_passes_only_when_every_step_passes() {
    let d = repo();
    let (outcome, attempts, _, _) = run(&d, "true\ntrue\n");
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(attempts, 1);
    let (outcome, _, _, _) = run(&d, "true\nfalse");
    assert_eq!(outcome, Outcome::GateFailed);
}

#[test]
fn a_single_line_gate_is_reported_as_before() {
    let d = repo();
    let (_, _, _, last) = run(&d, "echo plain && exit 3");
    assert!(last.contains("exit 3"), "{last:.600}");
    assert!(last.contains("plain"));
    assert!(!last.contains("steps failed"));
}

#[test]
fn the_system_prompt_shows_the_steps_as_one_command() {
    let d = repo();
    let (_, _, sys, _) = run(&d, "cargo fmt --check\ncargo test");
    assert!(sys.contains("`cargo fmt --check && cargo test`"), "{sys:.800}");
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_gate_steps`. Expected: failures (today the lines run as one shell script whose exit status is the last line's, and the prompt shows the raw newline).

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/run.rs`:
  1. Rename the existing `fn run_gate(&self, state: &State) -> (bool, String)` to `fn run_step(&self, cmd: &str) -> (bool, String)`, with the same body except that it executes `cmd` instead of `&state.orchestrator.verification_cmd`. Doc comment: `/// Run one gate command: exit status and its bounded output.`
  2. Add a new `run_gate` above it:

```rust
    /// Run the verification command. Runtime-only: nothing in the tool surface reaches this.
    ///
    /// A gate of several lines is several steps. Every step runs even after one fails, and all
    /// failures come back in one message: with `fmt && clippy && test` a shadow run found the
    /// clippy failure only after fixing the fmt one, a whole extra gate round of 7 turns.
    fn run_gate(&self, state: &State) -> (bool, String) {
        let steps = gate_steps(&state.orchestrator.verification_cmd);
        if steps.len() <= 1 {
            return self.run_step(&state.orchestrator.verification_cmd);
        }
        let failures: Vec<String> = steps
            .iter()
            .filter_map(|step| {
                let (passed, output) = self.run_step(step);
                (!passed).then(|| format!("$ {step}\n{output}"))
            })
            .collect();
        if failures.is_empty() {
            return (true, "exit 0".into());
        }
        let body = tools::bound_output(&failures.join("\n\n"));
        (
            false,
            format!("{} of {} steps failed\n\n{body}", failures.len(), steps.len()),
        )
    }
```

  3. Add a free function after the `impl<'a> Runner<'a>` block:

```rust
/// The steps of a gate: its non-blank lines, trimmed.
fn gate_steps(gate: &str) -> Vec<&str> {
    gate.lines().map(str::trim).filter(|s| !s.is_empty()).collect()
}
```

  4. In `system_prompt`, change `verify = state.orchestrator.verification_cmd,` to `verify = gate_steps(&state.orchestrator.verification_cmd).join(" && "),`.
  5. In `crates/blyte-cli/src/cli.rs`, change the `--gate` doc comment to: `/// Verification gate command (e.g. 'pytest', 'cargo test'). Several steps may be given one per line: every step runs and all failures are reported together.`

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_gate_steps --test test_run`. Expected: all pass (5 new).

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): a multi-line gate runs every step and reports all failures at once"
```
