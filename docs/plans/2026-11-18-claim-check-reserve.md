# Reserve time for the claim check in research runs

**Date:** 2026-11-18

## Why

A research run that uses its whole time budget in the main loop leaves nothing for the claim check
that runs afterwards. On 2026-11-18 run R-DA (1800 s budget, `budget_exhausted` after 56 turns)
recorded `claim_check_skipped: "out of time after 0 of 3 batches"`: the quote check found 13 problems
but none of the report's prose claims were checked, and the report contained statements quoted from
files that do not hold them.

## Decision

In a research run with the claim check on and a time budget, the main loop stops early by a reserve,
and the claim check that follows may use the time up to the full budget. The run's total time does not
change, so the dispatch watchdog needs no change.

- Reserve: 10% of `max_duration`, at most 180 s.
- Applies only when `research && claim_check && max_duration.is_some()`. Every other run keeps its
  whole budget for the loop.
- No fallback: when the reserve is still not enough, the claim check records `claim_check_skipped`
  exactly as it does today. Nothing is reported as checked that was not.

## Facts checked (at `899298e`)

- `Runner::run` (`crates/blyte-agent/src/run.rs`) computes one `deadline` from `budget.max_duration`
  and passes it to `run_until`, which passes the same value to `run_loop` and, after the loop, to
  `run_claim_check(deadline)`.
- Inside `run_loop` the claim check also runs when the gate passes (`self.run_claim_check(deadline)`
  near line 1509), using the loop's deadline.
- `claim_check::check_claims` sets each request's timeout to the time left before the deadline it is
  given and records `out of time after N of M batches` when it passes.
- `crates/blyte-agent/tests/test_deadline_stop.rs` has `ScriptedProvider`, which records each
  request's timeout in `seen_timeouts`, and the test
  `research_run_whose_deadline_passes_before_claim_check_sets_skipped_and_runs_quote_check`.

### Task CR1: the main loop of a research run leaves a reserve for the claim check

**Goal:** a research run with the claim check on stops its main loop a reserve before its time
budget, and the claim check may use the time up to the full budget, so a run that runs out of time
still gets its claims checked.

**Depends on:** none. **Siblings:** none.

**Files:**
- `crates/blyte-agent/src/run.rs`
- `crates/blyte-agent/tests/test_deadline_stop.rs`

**What to build:**

1. In `run.rs`, a public constant and a public pure function:

   ```rust
   /// Most time a research run's main loop leaves for the claim check.
   pub const CLAIM_CHECK_RESERVE_MAX: Duration = Duration::from_secs(180);

   /// Time the main loop of a research run leaves before its deadline for the claim check:
   /// a tenth of the time budget, at most `CLAIM_CHECK_RESERVE_MAX`.
   pub fn claim_check_reserve(max_duration: Duration) -> Duration
   ```

   `claim_check_reserve(d)` is `min(d / 10, CLAIM_CHECK_RESERVE_MAX)`.

2. In `Runner::run_until`, when `self.research && self.claim_check` and both `deadline` and
   `self.budget.max_duration` are `Some`, compute
   `loop_deadline = deadline - claim_check_reserve(max_duration)` and pass `loop_deadline` to
   `run_loop` in place of `deadline`. In every other case `run_loop` gets `deadline` unchanged.
   Use `Instant::checked_sub`; if it returns `None`, pass `deadline` unchanged (the reserve is never
   more than a tenth of the budget, so this cannot happen for a deadline built from `now + budget`;
   it is not a fallback for an unknown value).

3. The claim check uses the full deadline in both places it runs:
   - after the loop in `run_until`: keep passing the original `deadline` (not `loop_deadline`);
   - inside `run_loop` when the gate passes: it must also get the full deadline. Give `run_loop`
     what it needs to do that (for example an extra parameter `claim_deadline: Option<Instant>`);
     everything else in `run_loop` (the per-turn deadline test, model-call and command timeouts, the
     gate, `elapsed_fraction`, the "You have about N minutes left" message) uses the loop deadline it
     was given, as now.

4. Nothing else changes: the outcome of a run that reaches the loop deadline is still
   `BudgetExhausted`; no new report field; no change to `claim_check.rs`.

**Tests** (in `crates/blyte-agent/tests/test_deadline_stop.rs`, reusing its `ScriptedProvider`; run
with `cargo test -p blyte-agent --test test_deadline_stop`):

1. `claim_check_reserve_is_a_tenth_capped_at_180s`:
   `claim_check_reserve(Duration::from_secs(1800)) == Duration::from_secs(180)`;
   `claim_check_reserve(Duration::from_secs(600)) == Duration::from_secs(60)`;
   `claim_check_reserve(Duration::from_secs(7200)) == Duration::from_secs(180)`;
   `claim_check_reserve(Duration::from_secs(1)) == Duration::from_millis(100)`.

2. `research_loop_stops_a_reserve_before_the_budget`: a `Runner` with `research: true`,
   `claim_check: true`, `max_duration: Some(Duration::from_secs(100))`, `max_turns: 1`, and a
   `ScriptedProvider` with one turn (text only, no tool calls) and no sleep. After the run, the first
   entry of `seen_timeouts` is `Some(t)` with `t <= Duration::from_secs(90)` and
   `t > Duration::from_secs(80)`.

3. `non_research_loop_keeps_the_whole_budget`: the same with `research: false, claim_check: false`:
   the first entry is `Some(t)` with `t > Duration::from_secs(90)` and `t <= Duration::from_secs(100)`.
   And once more with `research: true, claim_check: false`: also `t > Duration::from_secs(90)`.

4. `claim_check_runs_in_the_reserve_after_the_loop_runs_out`: `research: true`, `claim_check: true`,
   `max_duration: Some(Duration::from_secs(2))` (reserve 200 ms, loop deadline 1.8 s). The workspace
   is set up as in the existing test `research_run_whose_deadline_passes_before_claim_check_...`
   (a `src.py`, a `report.md` with the prose claim citing `src.py:1-2`, `git init` and `git add`).
   The provider's first call (the loop turn, a `read` tool call) sleeps 1850 ms, so the loop deadline
   has passed when it returns but the full deadline has not; its later calls do not sleep and the
   second call returns a valid claim-check verdict reply for one claim (take the reply format from
   `parse_verdict_reply` and the tests in `crates/blyte-agent/src/claim_check.rs`). `ScriptedProvider`
   sleeps the same time before every call today: change it so the sleep can apply to the first call
   only, without changing what the existing tests do. Assert: the outcome is
   `Outcome::BudgetExhausted`; `report.claim_check_skipped` is `None`; `report.claim_check` is `Some`
   with `checked == 1`.

5. The existing test
   `research_run_whose_deadline_passes_before_claim_check_sets_skipped_and_runs_quote_check` passes
   unchanged in what it asserts (its provider sleeps past the full deadline, so the check is still
   skipped).

Before implementing, confirm tests 2 and 4 fail on the current code.

**Constraints:** no fallbacks (a claim check that cannot finish is recorded as skipped, as now; never
report unchecked claims as checked). Existing tests pass with their assertions unchanged. Not a
long-running job, so nothing to make resumable. Do not edit documentation: it is updated separately.

**Commit message:** `fix(research): the main loop leaves time for the claim check`
