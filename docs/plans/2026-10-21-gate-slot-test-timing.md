# Make the gate-slot deadline test independent of runner speed

**Context (2026-10-21):** `a_gate_waiting_for_a_slot_past_max_duration_still_runs_and_verifies`
(`crates/blyte-agent/tests/test_gate_slot_deadline.rs`) failed twice in a row on the Ubuntu CI runner with
`left: BudgetExhausted, right: Verified`, and passes on macOS and locally (about 1.6 s). Two causes in the test:

1. **Budget too tight.** The run gets `max_duration: 600 ms` while the slot is held for 800 ms. Slot waiting is
   excluded from the deadline (`run.rs`, `*d += Duration::from_millis(wait_ms)` after the gate), but everything else
   counts: building the system prompt and the work before the first turn's deadline check
   (`if deadline.is_some_and(|d| Instant::now() >= d) { return (Outcome::BudgetExhausted, ..) }`). On a slow runner
   that work alone can pass 600 ms, so the run stops before the gate.
2. **Shared environment.** The other test in the same binary,
   `the_default_is_a_quarter_of_the_cores_and_at_least_two`, sets and removes `BLYTE_GATE_SLOTS` while this test
   relies on it being `1`. Tests in one binary run on parallel threads; a `sleep(100 ms)` at the start of this test
   stands in for real exclusion.

### Task GS1: no wall-clock margin and no shared-env race in test_gate_slot_deadline

**Goal:** the test proves the same thing (a gate that waits for a slot for longer than `max_duration` still runs and
verifies, and the wait is recorded) without depending on how fast the machine is.

**Files:**
- `crates/blyte-agent/tests/test_gate_slot_deadline.rs` (only this file)

**What to build:**

1. Add a file-level `static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());`. Both tests take it at their
   start (`let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());`) and hold it to the end, so neither sees
   the other's environment changes. Remove the `std::thread::sleep(Duration::from_millis(100))` at the start of the
   slot test.
2. In the slot test, change the timings so the non-waiting work has ample room and the wait is still longer than the
   budget:
   - `max_duration: Some(Duration::from_secs(3))`;
   - the releaser thread drops the held slot after `Duration::from_secs(4)`;
   - the assertions become `start.elapsed() >= Duration::from_secs(3)` and `report.gate_wait_ms[0] >= 3000`, keeping
     their current messages.
   Keep `assert_eq!(outcome, Outcome::Verified)` and every other part of the test (the provider, the git setup, the
   restoring of the three environment variables) as it is.
3. Put the `Duration` constants for the budget and the hold time in two named `const`s at the top of the test
   (`BUDGET`, `HOLD`) with a one-line comment: the hold is longer than the budget so the gate waits past
   `max_duration`, and the budget is large so slow runners finish the non-waiting work inside it.

**Tests:** this task changes a test. Run `cargo test -p blyte-agent --test test_gate_slot_deadline` ten times in a row
(`for i in $(seq 10); do cargo test -q -p blyte-agent --test test_gate_slot_deadline || exit 1; done`); all must
pass. Then the full gate (the repository's `blyte.toml`).

**Constraints:** no other files change; no changes to `crates/blyte-agent/src/`; no fallbacks; do not mark the test
`#[ignore]` or add retries.

**Depends on:** none.

**Commit message:**
```
test(agent): make the gate-slot deadline test independent of runner speed

The test gave the run 600 ms and held the slot for 800 ms. The wait is
excluded from the deadline, but the work before the first turn is not,
and on a slow CI runner it passed 600 ms, so the run stopped with
BudgetExhausted before the gate. Give it a 3 s budget and hold the slot
for 4 s, and serialise the two tests that change BLYTE_GATE_SLOTS with
a lock instead of a sleep.
```
