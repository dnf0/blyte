# The Ungated Prompt — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** When blyte runs without a gate, its system prompt tells the model the truth: no command
checks the work, the result is judged by tests it cannot see, so it must verify the change itself
(the project's existing test suite, the tests covering the change, and the edge cases the task
describes) before stopping. The gated prompt is unchanged.

**Why (FrontierHarness run, 2026-09-26):** that benchmark runs blyte ungated. The system prompt,
written for gated runs, told the model "Your work is accepted when `` exits 0 … you do not decide the
verdict … leave the slow full run to it … simply stop when you believe the command will pass" — an
empty command, and an instruction not to run the full suite. Three DeepSWE tasks then failed on one
or two edge-case tests each (`anko` 7/9, `fastapi` 136/137, `httpx` 121/122 target tests, with every
existing test passing): the model stopped without the full run that would have shown them.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified (they check the gated prompt: "you do not decide the verdict",
  "run its quick parts yourself", "formatting, lint", the gate command, "Change nothing outside the
  repository").
- Only the `## How completion is decided` section changes, and only when the gate is empty (no
  non-blank line). Everything else in the prompt stays the same for both cases.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: An honest completion section for ungated runs

**Files:** `crates/blyte-agent/src/run.rs`; new test `crates/blyte-agent/tests/test_ungated_prompt.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_ungated_prompt.rs`:

```rust
//! Ungated, no command checks the work, so the prompt must not promise one or tell the model to
//! leave the full test run to it: three benchmark tasks failed on one or two edge-case tests after
//! the model stopped without running the suite.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    seen: Mutex<Vec<Request>>,
}

impl Provider for Stopper {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response::default())
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

fn system_prompt(gate: &str) -> String {
    let d = tempfile::tempdir().unwrap();
    let p = Stopper { seen: Mutex::new(Vec::new()) };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget { max_turns: 1, max_gate_attempts: 1, max_duration: None },
        rollback: false,
        on_event: None,
    };
    r.run(&mut State::new("g", gate));
    let sys = p.seen.lock().unwrap()[0].system.clone();
    sys
}

#[test]
fn ungated_the_prompt_says_nothing_checks_the_work() {
    let sys = system_prompt("");
    assert!(sys.contains("No command checks your work"), "{sys}");
    assert!(sys.contains("tests you cannot see"), "{sys}");
    assert!(sys.contains("run the project's existing test suite"), "{sys}");
    assert!(sys.contains("edge cases"), "{sys}");
}

#[test]
fn ungated_the_prompt_promises_no_command_and_no_full_run_for_you() {
    let sys = system_prompt("  \n ");
    assert!(!sys.contains("``"), "an empty command must not appear: {sys}");
    assert!(!sys.contains("leave the slow full run"), "{sys}");
    assert!(!sys.contains("you do not decide the verdict"), "{sys}");
}

#[test]
fn gated_the_prompt_is_unchanged() {
    let sys = system_prompt("cargo test");
    assert!(sys.contains("`cargo test`"), "{sys}");
    assert!(sys.contains("leave the slow full run to it"), "{sys}");
    assert!(!sys.contains("No command checks your work"), "{sys}");
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_ungated_prompt`. Expected: the two ungated tests fail; the gated one passes.

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/run.rs`, `system_prompt`: when
  `gate_steps(&state.orchestrator.verification_cmd)` is empty, use this `## How completion is
  decided` section instead of the gated one:

```
             No command checks your work. When you stop, the result is judged by tests you \
             cannot see, so verify it yourself first: run the project's existing test suite and \
             the tests that cover your change, and check the edge cases the task describes. Stop \
             only when you are confident the task is complete.\n\n\
```

  Keep the rest of the prompt identical in both cases. Add a one-line comment saying why (the Why
  above, briefly).

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_ungated_prompt --test test_run --test test_prompt_quick_checks --test test_stay_in_repo`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "fix(agent): ungated, the prompt says nothing checks the work and asks for a full test run"
```
