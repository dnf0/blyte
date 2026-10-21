# Fix runs that make no progress: nudge, then stop

**Why.** An RO shadow fix run on Gemini Argon (Sentry 141534646, 2026-10-21) used its whole 40-minute
limit: 109 turns, 2402 s of model time, 6 s of tools, no edits, no gate attempt. It printed library
source with `bash` (`python3 -c "…inspect.getsource(…)"`) and re-read files. The error was a transient
network failure that cannot be reproduced. The task said "make no change and explain", but nothing in the
run asked the model to decide. Two gaps in `Runner::run_loop` (`crates/blyte-agent/src/run.rs`):

1. No fix-run nudge exists for "no edit and no gate attempt for a long time" (the orchestrator has one,
   `ORCHESTRATOR_EXPLORE_NUDGE`; research has the report reminder).
2. The "10 turns left" warning (`run.rs` ~840) counts turns only, so a run stopped by the time limit
   (`deadline`) is never warned; at 15–90 s a turn, 40 minutes ends long before `max_turns - 9`.

**Design.**
- A fix run (`!self.research`, `state.orchestrator.instructions.is_none()`,
  `state.runtime.conversation.is_none()`) has *made progress* once any of these happened: a successful
  `edit`/`write` call, a workspace fingerprint (`self.fingerprint()`) different from the one taken before
  turn 1, or a gate attempt (`report.gate_attempts > 0`). Once it has, the detector is off for the run.
- **Nudge** (once): at the top of a turn, with no progress yet, when `turn > 20`, or when `turn > 8` and
  at least 25% of `self.budget.max_duration` has elapsed. The time check uses the live `deadline`
  (which already absorbs gate-slot waits): elapsed = `max_duration - (deadline - now)`.
- **Last call** (once, after the nudge): still no progress and either 10 turns since the nudge or at
  least 50% of `max_duration` elapsed: tell the model to reply now with its explanation and no tool calls.
- **Stop**: if the turn after the last call still calls tools and there is still no progress, return
  `Outcome::BudgetExhausted` with `report.stalled = true`. A model that stops calling tools ends the
  normal way (gate on the untouched tree → `verified` → `no_changes` in dispatch), with its explanation
  as `final_message`. No new outcome; RO's mapping is unchanged.
- **Time-aware wrap-up**: the existing wrap-up message also fires (once, for every kind of run) when the
  time left is under 10% of `max_duration`; the turn-based one keeps working and the two never both fire.

No fallbacks: when `max_duration` or `deadline` is `None`, the time-based conditions do not apply and only
the turn-based ones do (that is the documented meaning of "no time limit", not a stand-in).

### Task NP1: no-progress nudge, last call and stop for fix runs

**Depends on:** none. **Siblings:** NP2 (time-aware wrap-up) runs after this on its branch; do not add it here.

**Files:** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/report.rs`,
new `crates/blyte-agent/tests/test_no_progress.rs`.

**Build.**
- In `run.rs`, constants next to `ORCHESTRATOR_EXPLORE_NUDGE`:
  `NO_PROGRESS_TURNS: u32 = 20`, `NO_PROGRESS_MIN_TURNS: u32 = 8`, `NO_PROGRESS_TIME_FRACTION: f64 = 0.25`,
  `LAST_CALL_TURNS: u32 = 10`, `LAST_CALL_TIME_FRACTION: f64 = 0.5`.
- In `run_loop`, track `start_fp = self.fingerprint()` before turn 1 and a `progressed: bool` set when a
  successful `edit`/`write` runs, when a fingerprint taken in the loop differs from `start_fp`, or when a
  gate attempt starts. Track `nudge_turn: Option<u32>`, `last_call_sent: bool`.
- At the top of each turn (beside the existing nudges, after the deadline and cost checks), for fix runs
  only (definition in Design) and `!progressed`:
  - if `nudge_turn.is_none()` and (`turn > NO_PROGRESS_TURNS` or (`turn > NO_PROGRESS_MIN_TURNS` and
    elapsed fraction ≥ `NO_PROGRESS_TIME_FRACTION`)): push exactly
    `"[blyte] You have spent {n} turns without changing any file or running the gate. Stop exploring and decide now: make the change the task asks for, or, if the task needs no code change or the problem cannot be reproduced, stop calling tools and explain why in your reply."`
    where `{n}` is `turn - 1`; set `nudge_turn = Some(turn)`.
  - else if `nudge_turn` is `Some(t)`, `!last_call_sent` and (`turn >= t + LAST_CALL_TURNS` or elapsed
    fraction ≥ `LAST_CALL_TIME_FRACTION`): push exactly
    `"[blyte] Still no change after the last reminder. Reply now with your conclusion and why, and do not call any tools: the run stops if this reply calls a tool without changing anything."`
    and set `last_call_sent = true`.
  - else if `last_call_sent` and the previous turn (the one answering the last call) called a tool and
    there is still no progress: set `report.stalled = true` and return `(Outcome::BudgetExhausted, report)`.
- Elapsed fraction: `None` when `self.budget.max_duration` or `deadline` is `None`; otherwise
  `(max_duration - deadline.saturating_duration_since(now)) / max_duration`. Write it as a small private
  helper.
- `RunReport` gets `pub stalled: bool` (doc comment: run stopped because a fix run made no change and kept
  calling tools after the last call), and `crates/blyte-cli/src/report.rs` carries it into the JSON report
  exactly as `fold_thrash` is carried (struct field, `false` default, copied from the run report).
- Research, orchestrator and conversational runs never see either message and never stop for this.
- No fallbacks.

**Tests** (`crates/blyte-agent/tests/test_no_progress.rs`, following `tests/test_turns_left.rs` and
`tests/test_fold_thrash.rs`: a `Provider` that records each request's messages and returns a tool call):
1. A fix run whose provider always calls `bash` `true` (no change) with `max_turns = 60` and no time limit:
   the nudge text first appears in the request for turn 21, exactly once; the last-call text first appears
   in the request for turn 31, exactly once; the run ends with `Outcome::BudgetExhausted`,
   `report.stalled == true`, and the provider was called exactly 31 times (the stop happens at the top of turn 32, before the model call).
2. Same, but the provider replies with text and no tool calls once it sees the last-call text: the run ends
   through the normal stop path (not `BudgetExhausted`), `stalled == false`, and `final_message` is that
   text. Use an empty gate (`verification_cmd` empty) so the stop returns `Verified`.
3. A provider that does one successful `write` of a new file on turn 3 and then calls `bash` `true`
   forever, `max_turns = 60`: neither message ever appears, `stalled == false`, outcome `BudgetExhausted`
   at turn 60.
4. Research run (`research = true` the way existing research tests set it) with the always-`bash`
   provider and `max_turns = 40`: neither message appears, `stalled == false`.
5. Time trigger: provider sleeps 100 ms per call and always calls `bash` `true`; `max_turns = 200`,
   `max_duration = 3 s` with a matching deadline. The nudge appears exactly once, in a request for a turn
   in 10..=20 (25% of 3 s is after ~8 turns of 100 ms; the window is wide on purpose so slow CI machines
   still pass), and the last call appears before the run ends.
Run: `cargo test -p blyte-agent --test test_no_progress`; the full gate must pass.

**Constraints:** existing tests pass unmodified. Keep the existing nudges' text and order.

**Commit message:** `feat(agent): nudge, then stop, fix runs that make no progress`

### Task NP2: time-aware wrap-up warning

**Depends on:** NP1 (same region of `run_loop`; dispatch with `--base wip/NP1`).

**Files:** `crates/blyte-agent/src/run.rs`, new `crates/blyte-agent/tests/test_time_left.rs`.

**Build.**
- Keep the turn-based warning at `turn == max_turns - 9` exactly as it is.
- Add, for every kind of run: once, at the top of a turn, when `self.budget.max_duration` and `deadline`
  are both `Some` and the time left (`deadline.saturating_duration_since(now)`) is under 10% of
  `max_duration`, push exactly
  `"You have about {m} minutes left of {total}. Wrap up now: make sure your work is saved before you run out: committed, or written to the file the task asks for. Then stop calling tools."`
  where `{m}` is the time left rounded up to whole minutes (minimum 1) and `{total}` is `max_duration` in
  whole minutes rounded up.
- A single `wrap_up_sent: bool` covers both triggers: whichever fires first sends its message, the other
  never fires afterwards.
- No time limit means no time-based warning (documented meaning, not a fallback). No fallbacks.

**Tests** (`crates/blyte-agent/tests/test_time_left.rs`, pattern of `tests/test_turns_left.rs`):
1. Provider sleeps 100 ms per call and calls `bash` `true`; research run (so NP1's messages do not
   interfere), `max_turns = 300`, `max_duration = 2 s` with a matching deadline: the time warning appears
   in exactly one request, and the turn-based "You have 10 turns left" text never appears.
2. No time limit, `max_turns = 30`: the existing turn-based warning appears once at turn 21 and the time
   warning never appears (same as `test_turns_left`).
3. `max_turns = 25`, `max_duration = 600 s`, fast provider: only the turn-based warning appears, once.
Run: `cargo test -p blyte-agent --test test_time_left`; the full gate must pass.

**Constraints:** `tests/test_turns_left.rs` passes unmodified.

**Commit message:** `feat(agent): warn time-limited runs before the deadline`
