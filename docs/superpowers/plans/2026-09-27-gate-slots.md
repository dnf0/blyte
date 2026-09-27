# Gate Slots: Parallel Runs Share the Machine — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** parallel dispatches keep implementing in parallel, but their **gates** (the heavy part:
full builds and test suites) take a machine-wide slot, so three runs do not all start
`cargo test` in three worktrees at once and slow each other down.

**Why (2026-09-27):** parallel dispatch is now the default. In a Rust repository each worktree
has its own `target/`, so three simultaneous gates are three full builds competing for the same
cores. A slot limit (default: an eighth of the cores, at least one; see the measurement in `gate_slots.rs`) queues gates briefly instead.

**Tech Stack:** Rust, `libc::flock` (`libc` is already a unix dependency of `blyte-agent`).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**, with one allowance: literal `RunReport { … }`
  constructions in existing tests may gain `gate_wait_ms: Vec::new()`, nothing else.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Machine-wide gate slots

**Files:** `crates/blyte-agent/src/gate_slots.rs` (new), `crates/blyte-agent/src/lib.rs`,
`crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/report.rs`,
`crates/blyte-agent/tests/test_gate_slots.rs` (new), and the named allowance.

**What to build:**

- `gate_slots.rs` (`pub mod gate_slots;`):
  - `pub fn slots() -> usize`: `BLYTE_GATE_SLOTS` when it parses to an integer ≥ 1, otherwise
    `max(1, std::thread::available_parallelism() / 8)` (measured default; originally `/ 4`) (1 if that call fails).
  - `pub fn slot_dir() -> PathBuf`: `BLYTE_GATE_SLOT_DIR` when set and non-empty, otherwise
    `std::env::temp_dir().join("blyte-gate-slots")`.
  - `pub struct SlotGuard { … }` holding the open lock file (and `pub fn index(&self) -> usize`);
    dropping it releases the slot (closing the file releases the `flock`).
  - `pub fn acquire(max: usize, dir: &Path, poll: Duration) -> std::io::Result<(SlotGuard, Duration)>`:
    create `dir`; repeatedly try `slot-0.lock` … `slot-<max-1>.lock` (open with create, then
    `flock(fd, LOCK_EX | LOCK_NB)`); return the first slot taken and how long it waited in total;
    when all are held, sleep `poll` and try again. On non-unix targets return a guard at once with
    zero wait.
- `run.rs`, `run_gate`: before running the gate's steps, `acquire(slots(), &slot_dir(), 500ms)`.
  If acquiring fails (an I/O error), run the gate anyway and say why on stderr. When it had to
  wait, print once on stderr: `gate: waited <n>s for a gate slot (<max> in use)`. Hold the guard for
  all steps of that gate attempt; release it when the attempt ends.
- `RunReport` gains `pub gate_wait_ms: Vec<u64>` (one entry per gate attempt, 0 when a slot was
  free at once). `RunRecord` (the `--report` JSON) gains `gate_wait_ms`, copied in `from_run`,
  empty in `from_error`. `SCHEMA_VERSION` stays 1.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_gate_slots.rs`:

```rust
//! Parallel dispatches share the machine: gates take a machine-wide slot, so three runs do not
//! all start a full build and test suite at once (2026-09-27).

use std::time::{Duration, Instant};

use blyte_agent::gate_slots::{acquire, slots};

const POLL: Duration = Duration::from_millis(20);

#[test]
fn one_slot_makes_the_second_gate_wait_for_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let (first, waited) = acquire(1, dir.path(), POLL).unwrap();
    assert!(waited < Duration::from_millis(100));
    let d = dir.path().to_path_buf();
    let second = std::thread::spawn(move || {
        let start = Instant::now();
        let (_g, waited) = acquire(1, &d, POLL).unwrap();
        (start.elapsed(), waited)
    });
    std::thread::sleep(Duration::from_millis(400));
    drop(first);
    let (elapsed, waited) = second.join().unwrap();
    assert!(elapsed >= Duration::from_millis(350), "{elapsed:?}");
    assert!(waited >= Duration::from_millis(300), "{waited:?}");
}

#[test]
fn two_slots_let_two_gates_run_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (a, _) = acquire(2, dir.path(), POLL).unwrap();
    let (b, waited) = acquire(2, dir.path(), POLL).unwrap();
    assert!(waited < Duration::from_millis(100), "{waited:?}");
    assert_ne!(a.index(), b.index());
}

#[test]
fn a_released_slot_is_reused() {
    let dir = tempfile::tempdir().unwrap();
    let (a, _) = acquire(1, dir.path(), POLL).unwrap();
    drop(a);
    let (_b, waited) = acquire(1, dir.path(), POLL).unwrap();
    assert!(waited < Duration::from_millis(100));
}

// The only test in this binary that reads or sets BLYTE_GATE_SLOTS.
#[test]
fn the_slot_count_defaults_to_at_least_one_and_can_be_set() {
    std::env::remove_var("BLYTE_GATE_SLOTS");
    assert!(slots() >= 1);
    std::env::set_var("BLYTE_GATE_SLOTS", "3");
    assert_eq!(slots(), 3);
    std::env::set_var("BLYTE_GATE_SLOTS", "0");
    assert!(slots() >= 1, "0 falls back to the default");
    std::env::remove_var("BLYTE_GATE_SLOTS");
}
```

Also add to an existing-style runner test (new file `crates/blyte-agent/tests/test_gate_wait_report.rs`)
a run with a scripted provider that stops at once and a gate `true`, with `BLYTE_GATE_SLOT_DIR`
set to a temporary directory, asserting `report.gate_wait_ms.len() == report.gate_attempts as usize`.
Build the `Runner` exactly as `crates/blyte-agent/tests/test_provider_retry.rs` does.

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_gate_slots`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): gates take a machine-wide slot, so parallel runs do not all build and test at once"
```
