# Run the Quick Checks Before Stopping — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** blyte's system prompt tells the model to run the gate's quick parts (formatting, lint) and the tests covering its change before it stops, leaving only the slow full run to the gate. It no longer says the model "cannot run it as your final step", which read as "do not run it at all".

**Why (shadow runs S1–S3, 2026-09-25):** every blyte gate failure so far was one the model could have caught in seconds: S1 failed on `cargo fmt` and then on `clippy`, S3 failed on `cargo fmt` alone. Each failed gate costs a full rerun of the suite (about 45–100 s here) plus the turns to recover. The model never formatted before stopping, because the prompt told it the gate is not its to run.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Quick checks in the system prompt

**Files:** `crates/blyte-agent/src/run.rs`; new test `crates/blyte-agent/tests/test_prompt_quick_checks.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_prompt_quick_checks.rs`:

```rust
//! The system prompt asks for the gate's quick parts before stopping: every gate failure in
//! shadow runs S1-S3 was a formatting or lint error the model could have caught in seconds,
//! and each cost a full rerun of the suite.

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

fn system_prompt() -> String {
    let d = tempfile::tempdir().unwrap();
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
            max_turns: 1,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    r.run(&mut State::new("g", "true"));
    let sys = p.seen.lock().unwrap()[0].system.clone();
    sys
}

#[test]
fn the_prompt_asks_for_the_quick_checks_before_stopping() {
    let sys = system_prompt();
    assert!(sys.contains("run its quick parts yourself"), "{sys}");
    assert!(sys.contains("formatting, lint"), "{sys}");
}

#[test]
fn the_prompt_no_longer_reads_as_a_ban_on_checking() {
    let sys = system_prompt();
    assert!(!sys.contains("cannot run it as your final step"), "{sys}");
    assert!(sys.contains("you do not decide the verdict"));
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_prompt_quick_checks`. Expected: both tests fail.

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/run.rs`, in `system_prompt`, replace the `## How completion is decided` paragraph, which currently reads:

```
             Your work is accepted when `{verify}` exits 0. That command is run for you when \
             you stop calling tools -- you cannot run it as your final step and you do not \
             decide the verdict. Do not claim to be finished; simply stop when you believe the \
             command will pass, and you will be told.\n\n\
```

with:

```
             Your work is accepted when `{verify}` exits 0. That command is run for you when \
             you stop calling tools, and you do not decide the verdict. A failure costs a full \
             rerun after your fix, so before you stop, run its quick parts yourself \
             (formatting, lint) together with the tests that cover your change; leave the slow \
             full run to it. Do not claim to be finished; simply stop when you believe the \
             command will pass, and you will be told.\n\n\
```

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_prompt_quick_checks --test test_run --test test_gate_steps`. Expected: all pass (2 new).

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): the prompt asks for the gate's quick checks before stopping"
```
