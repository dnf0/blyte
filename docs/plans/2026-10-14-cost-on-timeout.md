# Plan: report usage and cost when the watchdog stops a run

When `--max-seconds` (plus the grace period) passes, the watchdog thread in
`crates/blyte-cli/src/task_runner.rs` (`run_task`, around lines 146-193) writes
`RunRecord::from_error` and exits. That record has empty `usage` and `cost_usd: None`, so every
run stopped at its time limit reports $0 for all the tokens it already spent. In the Argon pair
pilot this hid the orchestrator's whole cost on 2 of 5 tasks.

### Task CT1

**Goal:** a run stopped by the watchdog reports the usage and cost it had accumulated up to its
last finished model call.

**Where:**
- `Progress` (`task_runner.rs` line 12) is the shared state the watchdog reads; it is `Copy`.
- The watchdog's record is built around line 185; `turns` and `gate_attempts` are copied in from
  `Progress` already.
- `RunRecord` is in `crates/blyte-cli/src/report.rs`: `usage: UsageRecord` (line 86) and
  `cost_usd` (line 77). See how the normal path fills both (around line 189:
  `blyte_provider::pricing(model_id).map(|p| p.cost(&report.usage))`).
- Find where the run loop updates `Progress.turns` after each model call, and update the
  accumulated usage at the same place.

**Change:**
- Keep the running total of token usage in `Progress` (or alongside it in the same mutex),
  updated after every model call, in whatever form lets the watchdog build a `UsageRecord`.
  Keep `Progress` cheap to copy.
- In the watchdog, fill the record's `usage` from it and compute `cost_usd` the same way the
  normal path does, with the run's model.
- Do not change the error message, the exit code, or anything else in the watchdog record.

**Tests:** extend the existing watchdog test (search the tests for `BLYTE_WATCHDOG_GRACE_SECS`)
or add one beside it: a run with a scripted provider that completes at least one model call
with known token counts, then hangs past `--max-seconds`; the written report has those token
counts in `usage` and a `cost_usd` greater than 0 for a priced model (or equal to the pricing
of those tokens).
