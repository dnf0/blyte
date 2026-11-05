# Plan: research runs write a report instead of stopping on a read loop

Spec: `docs/specs/2026-11-05-research-read-loop.md` (read it first; it is the authority). One task.

### Task RL1

Files:
- `crates/blyte-agent/src/run.rs`
- `crates/blyte-agent/tests/test_read_loop.rs` (existing read-loop tests; add the new ones here,
  following its scripted-provider style)

All changes apply only when `self.research` is true. Non-research runs must behave exactly as
today; an existing test must still prove that a non-research run stops at `READ_LOOP_LIMIT`.

1. Constants next to `READ_LOOP_LIMIT`, each with a doc comment:
   `RESEARCH_FIRST_REPORT_TURN: u32 = 8`, `READ_LOOP_WARN: u32 = 20`,
   `READ_LOOP_GRACE_TURNS: u32 = 3`.
2. Early reminder: next to the existing `turn % 30 == 0` research reminder, at turn
   `RESEARCH_FIRST_REPORT_TURN`, if `report.md` cannot be read or is empty (after trimming),
   push a user message telling the model to start `report.md` now with what it has found so far,
   even if unfinished, then carry on.
3. Warning: when `repeats` reaches `READ_LOOP_WARN` in a research run, append once (until
   `repeats` is reset) a `[blyte]` note to that tool result: it has re-read 20 files or searches
   since the workspace last changed, earlier results were dropped to save space, it should write
   what it needs into `report.md` now, and reading stops at 30. In research runs, the existing
   third-use warning ends with "write what you need into report.md" instead of "and make your
   change".
4. At `READ_LOOP_LIMIT` in a research run: set `report.read_loop = true` but do not return.
   Push a `[blyte]` user message: stop reading; write `report.md` now from what you have found,
   mark what you could not confirm as open questions; then stop calling tools. Record the turn.
   After that, any `read`, `grep` or `glob` call returns an error result (`is_error: true`)
   saying reading is closed and to write `report.md`; do not run it. `edit`/`write` still work.
   If the run has not ended after `READ_LOOP_GRACE_TURNS` more turns, return
   `(Outcome::BudgetExhausted, report)` as today. If the model stops calling tools within the
   grace turns, the normal research end path runs (report checks and any fix round it asks for,
   still within the grace turns). A `write` during grace must not reset the grace.
5. Tests (scripted provider, temp workspace), each research unless stated:
   - the turn-8 reminder is sent when `report.md` is missing, and not when it has content;
   - the 20-repeat warning is added once, and the third-use warning mentions `report.md`;
   - at 30 repeats the run does not end; a later `read` gets the "reading is closed" error;
     the model writes `report.md` and stops, and the run ends the normal research way with
     `read_loop` true;
   - at 30 repeats, a model that keeps calling tools for 3 more turns ends `BudgetExhausted`;
   - a non-research run still ends `BudgetExhausted` at 30 repeats with no grace.

No fallbacks.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
