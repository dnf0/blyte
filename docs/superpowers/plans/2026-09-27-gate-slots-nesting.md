# Gate Slots Must Not Deadlock on Themselves — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** gate slots (#119) never make blyte wait for itself. Only the `blyte` command takes
slots (not the library, so not tests); and a `blyte` started from inside a gate takes none.

**Why (2026-09-27):** with one slot (the default on 12 cores), a blyte run holding the slot ran
its gate, `cargo test` of this repository, whose runner tests run gates of their own and waited
for that same slot. The gate hung until its 30-minute deadline, and every other blyte gate on the
machine queued behind it: two superstate-eval dispatches waited 27 minutes each, and an agy run's
`cargo test` blocked the same way.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified** (`test_gate_slots.rs` calls `acquire` directly and is
  unaffected; `test_gate_wait_report.rs` still sees one `gate_wait_ms` entry per gate attempt, 0
  when slots are off).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Slots off by default in the library; nested runs skip them

**Files:** `crates/blyte-agent/src/gate_slots.rs`, `crates/blyte-agent/src/run.rs`,
`crates/blyte-cli/src/main.rs`, `crates/blyte-agent/tests/test_gate_slots_nesting.rs` (new).

**What to build:**

- `gate_slots.rs`:
  - `pub const IN_GATE_ENV: &str = "__BLYTE_IN_GATE";` (a name the command environment filter
    passes: it does not start with `BLYTE_` and has no credential-like segment).
  - `pub fn enable()` / `pub fn enabled() -> bool`: a process-wide switch (an `AtomicBool`,
    default **off**).
  - `pub fn nested() -> bool`: true when the environment variable `IN_GATE_ENV` is set (any value).
- `run.rs`, `run_gate`: take a slot only when `gate_slots::enabled() && !gate_slots::nested()`;
  otherwise skip acquisition (wait 0, still pushed to `gate_wait_ms`). `run_step` runs each gate
  command with `IN_GATE_ENV=1` in its environment for that command only, by sending the command
  as `(export __BLYTE_IN_GATE=1\n<command>\n)`, so a `blyte` or a test binary started by the gate
  sees it; later model commands in the same session do not.
- `main.rs`: call `blyte_agent::gate_slots::enable()` before running a headless task or the REPL
  (not for `init`, `skill`, `status` or `review`). `dispatch` needs nothing: its child `blyte`
  run enables slots itself.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_gate_slots_nesting.rs`:

```rust
//! Gate slots never make blyte wait for itself: a run holding the only slot ran `cargo test`,
//! whose runner tests waited for that slot until the gate's 30-minute deadline (2026-09-27).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::gate_slots::{acquire, enabled, nested, IN_GATE_ENV};
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    seen: Mutex<u32>,
}

impl Provider for Stopper {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        *self.seen.lock().unwrap() += 1;
        Ok(Response { text: "done".into(), ..Default::default() })
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

// One test: it sets process environment variables.
#[test]
fn the_library_takes_no_slot_and_gate_commands_are_marked() {
    assert!(!enabled(), "slots are off unless the blyte command turns them on");

    // Hold the only slot of a private slot directory, as a running blyte gate would.
    let slot_dir = tempfile::tempdir().unwrap();
    std::env::set_var("BLYTE_GATE_SLOT_DIR", slot_dir.path());
    std::env::set_var("BLYTE_GATE_SLOTS", "1");
    let (_held, _) = acquire(1, slot_dir.path(), Duration::from_millis(20)).unwrap();

    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec("git init -q && git config user.email a@b && git config user.name c && touch base && git add -A && git commit -qm base", d.path(), Duration::from_secs(30))
        .unwrap();
    let p = Stopper { seen: Mutex::new(0) };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget { max_turns: 3, max_gate_attempts: 1, max_duration: None },
        rollback: false,
        on_event: None,
    };
    // The gate records whether it saw the marker.
    let mut st = State::new("g", &format!("test -n \"${IN_GATE_ENV}\" && touch saw-marker"));
    let start = Instant::now();
    let (outcome, report) = r.run(&mut st);
    assert!(start.elapsed() < Duration::from_secs(20), "the run must not wait for the held slot");
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_wait_ms, vec![0]);
    assert!(d.path().join("saw-marker").exists(), "gate commands see {IN_GATE_ENV}");

    // Commands outside the gate do not see it.
    let out = LocalShell.exec(&format!("echo \"[${IN_GATE_ENV}]\""), d.path(), Duration::from_secs(30)).unwrap();
    assert_eq!(out.stdout.trim(), "[]");

    assert!(!nested());
    std::env::set_var(IN_GATE_ENV, "1");
    assert!(nested());
    std::env::remove_var(IN_GATE_ENV);
    std::env::remove_var("BLYTE_GATE_SLOT_DIR");
    std::env::remove_var("BLYTE_GATE_SLOTS");
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_gate_slots_nesting`: compile error.

- [ ] **Step 3: Implement** the changes above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "fix(agent): gate slots never deadlock: only the blyte command takes them, and not from inside a gate"
```
