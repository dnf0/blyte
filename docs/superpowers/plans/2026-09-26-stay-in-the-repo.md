# Stay in the Repository — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** blyte itself tells the model to change nothing outside the repository, and its failed-gate
message says to fix the change, not the machine. Today only the orchestrator's dispatch prompt says
this; anyone running blyte directly gets no such instruction.

**Why (shadow run E5, 2026-09-26):** after its first gate failure, blyte's model spent about 50 minutes
"freeing disk space" on the host (`uv cache clean`, `find ~/.cache/uv … | xargs …`) instead of stopping.
The failed-gate message says only "Verification did not pass", which reads as "make it pass by any
means", and the system prompt never states the workspace boundary.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified (they check for "Verification did not pass", "reverted",
  "Available:" and "you do not decide the verdict"; keep all of those).
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: The boundary in the system prompt and the failed-gate message

**Files:** `crates/blyte-agent/src/run.rs`; new test `crates/blyte-agent/tests/test_stay_in_repo.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_stay_in_repo.rs`:

```rust
//! The model is told the workspace boundary, and a failed gate asks for a fix to the change, not
//! to the machine: after a gate failure a model once spent 50 minutes deleting a host cache.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::{LocalShell, Shell};
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

fn run(gate: &str) -> (String, String) {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             touch x && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    let p = Stopper { seen: Mutex::new(Vec::new()) };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget { max_turns: 4, max_gate_attempts: 2, max_duration: None },
        rollback: false,
        on_event: None,
    };
    r.run(&mut State::new("g", gate));
    let seen = p.seen.lock().unwrap();
    (seen[0].system.clone(), format!("{:?}", seen.last().unwrap().messages))
}

#[test]
fn the_system_prompt_states_the_workspace_boundary() {
    let (sys, _) = run("true");
    assert!(sys.contains("Change nothing outside the repository"), "{sys}");
}

#[test]
fn a_failed_gate_asks_for_a_fix_to_the_change_not_the_machine() {
    let (_, last) = run("false");
    assert!(last.contains("Verification did not pass"), "{last:.400}");
    assert!(last.contains("fix your change, not the machine"), "{last:.600}");
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_stay_in_repo`. Expected: both fail.

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/run.rs`:
  1. In `system_prompt`, add a paragraph to the `## Environment` section, after the `Available:`
     line: `Change nothing outside the repository: no caches, other repositories, home-directory
     files or system settings. If something outside it blocks you (disk space, network, a missing
     tool), say so and stop.`
  2. In the failed-gate message, keep the existing text and add after the command output (before any
     reset note): `\n\nIf the failure is caused by your change, fix your change, not the machine. If
     it is not caused by your change (disk space, network, a missing service), say so and stop.`

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_stay_in_repo --test test_run --test test_prompt_quick_checks`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "fix(agent): blyte tells the model to change nothing outside the repository"
```
