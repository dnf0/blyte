# Record cost as each turn happens

Today a run writes one line to the usage ledger (`usage.jsonl`) when it ends, so a run or a
conversation that is killed leaves no record of what it spent. The ledger's only readers are in
this repository, all through `cost::totals` (`blyte cost`, the status line, the terminal
session's `/cost`).

### Task L1: one ledger line per turn, and a closing line per run

**Depends on:** none. **Siblings:** none.

**Goal.** Every model turn is in the ledger as soon as it ends, so a killed run keeps its cost;
totals for a finished run are exactly what they are today.

**Files.** `crates/blyte-cli/src/task_runner.rs`, `crates/blyte-cli/src/cost.rs`,
`crates/blyte-cli/src/usage_log.rs` (only if a helper belongs there), and tests in
`crates/blyte-cli/tests/test_usage_session.rs` and `crates/blyte-cli/tests/test_cost.rs` (add
tests; existing tests pass unmodified unless one asserts "exactly one ledger line per run", in
which case change that assertion only and say so in your report). No other file.

**What to build.**

1. In `run_task_with_event_callback` (`task_runner.rs`), on every `Event::Turn`, before the
   caller's `on_event` sees it, append a `usage_log::Entry` with `outcome: "turn"`, `turns: 1`,
   that turn's own token counts (first confirm in `blyte-agent/src/run.rs` whether the event's
   `usage` is the turn's own or a running total; if it is a running total, write the difference
   from the previous turn), and `cost_usd` priced from those counts. `ts`, `repo`, `session`,
   `dispatch`, `research` and `model` are filled exactly as the closing entry fills them
   today; build both from one function so they cannot drift.
2. When the run returns, the closing entry keeps today's `outcome` and carries only what the
   turn lines did not: `turns = report.outcome_turns - <turn lines written>`, each token count
   `report.usage.<field> - <sum written>`, and `cost_usd` priced from that remainder (`None`
   when the model has no price, as today). If any remainder would be negative, that is a bug
   in the accounting: write nothing for the closing entry's numbers that hides it; return the
   run's result as normal but print `error: usage log: turn lines exceed the run's total
   (<field>: <written> > <total>)` to stderr and write the closing entry with zeros.
3. A failed append is reported as today (`warning: usage log: <err>`), once per failure; it
   does not stop the run. No ledger path configured means no lines, as today.
4. `cost::totals`: `runs` counts entries that have a `dispatch` and whose `outcome` is not
   `"turn"`. Costs sum over every entry as now. A ledger written by an older blyte (one line
   per run, no `"turn"` lines) gives the same totals as before.

No fallbacks: no default stands in for a missing price or a missing value. Add no comments to
code. Nothing here needs to be resumable (each line is written as its turn ends, which is the
point).

**Tests.** Use whatever the existing tests in `test_usage_session.rs` use to run a task against
a scripted model and a temporary ledger (`cargo test -p blyte --test test_usage_session --test
test_cost`):

1. `each_turn_writes_a_line`: a three-turn run: three lines with `outcome == "turn"` and
   `turns == 1`, then one closing line with the run's outcome.
2. `lines_sum_to_the_run_total`: over all lines of that run, the sums of `turns`, `input`,
   `cached`, `output` and `thinking` equal the run report's values.
3. `turn_lines_are_on_disk_before_the_run_ends`: from the `on_event` callback of
   `run_task_with_state_and_events`, on the second turn, read the ledger: it already holds two
   `"turn"` lines.
4. `turn_lines_carry_session_and_dispatch`: with `BLYTE_SESSION` and `BLYTE_DISPATCH_ID` set
   (as the existing tests set them), every line has both.
5. In `test_cost.rs` or the unit tests of `cost.rs`: `turn_lines_do_not_count_as_runs` (three
   `"turn"` lines and one closing line for one dispatch: `runs == 1`, cost is the sum of all
   four); `old_ledger_totals_unchanged` (two closing lines only: `runs == 2`);
   `a_killed_run_keeps_its_cost` (two `"turn"` lines and no closing line: cost counted,
   `runs == 0`).

**Commit message:** `feat(cost): write each turn to the usage ledger as it ends`
