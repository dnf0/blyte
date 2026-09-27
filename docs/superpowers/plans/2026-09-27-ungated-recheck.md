# Ungated Runs Re-run Their Own Searches Before Stopping — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** with no gate, blyte's instructions tell the model to repeat the searches that found the
problems before it stops, and to confirm they now find nothing left to fix.

**Why (2026-09-27):** on FrontierHarness's `sanitize-git-repo` (ungated), blyte found five fake
secrets with `git grep`, fixed the ones in two files, then checked only for the strings it had
replaced before stopping. The two tokens left in the third file were still there, and the task
failed. Re-running the original discovery searches would have shown them.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified** (in particular `test_ungated_prompt.rs`).
- The gated prompt is unchanged.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: One sentence in the ungated completion text

**Files:** `crates/blyte-agent/src/run.rs`, `crates/blyte-agent/tests/test_ungated_recheck.rs` (new).

**What to build:** in `system_prompt`, the ungated `completion` text gains, directly after
`check the edge cases the task describes.`, the sentence:
`If you searched for problems (with grep or other commands), run those same searches again before you stop and confirm they find nothing left to fix.`
Everything else in the text stays as it is.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_ungated_recheck.rs`:

```rust
//! Ungated, the model re-runs the searches that found the problems before it stops: on
//! sanitize-git-repo it checked only the strings it had replaced, and two secrets it had found
//! in a third file were still there (2026-09-27).

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

const RECHECK: &str = "run those same searches again before you stop and confirm they find nothing left to fix";

#[test]
fn ungated_the_model_is_told_to_repeat_its_searches() {
    assert!(system_prompt("").contains(RECHECK));
}

#[test]
fn gated_the_prompt_is_unchanged() {
    assert!(!system_prompt("cargo test").contains(RECHECK));
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_ungated_recheck`: the first test fails.

- [ ] **Step 3: Implement** the sentence.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): ungated, blyte re-runs the searches that found the problems before it stops"
```
