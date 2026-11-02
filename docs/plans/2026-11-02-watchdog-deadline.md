# Plan: stop at the deadline instead of being killed by the watchdog (2026-11-02)

## Why

From a blyte research run over the 48 error runs' logs (RW1, report kept outside the repository; checked against the
code). In short:

- 46 of 323 runs (2026-10-19..23) were killed by the watchdog 30 s past `--max-seconds`.
  - 40 were in a model call (`provider.complete`, Gemini Argon) for the next turn.
  - 6 were research runs in the gate's claim check, which also makes model calls.
  - None were in a bash command.
- Nothing bounds a model call by the time left: `Request` (`crates/blyte-provider/src/lib.rs:189`) has no
  timeout, and the soft deadline is only checked at the top of a turn (`crates/blyte-agent/src/run.rs:852`).
  - Bash commands get a fixed `COMMAND_TIMEOUT` of 300 s (`tools.rs:295`).
  - The gate gets 1800 s (`run.rs:544`).
- The work itself mostly survives in the worktree: 42 of 46 runs left a usable report or edits. What is lost:
  - the run report is replaced by `RunRecord::from_error`: no turn usage, tool counts, model id, final message,
    quote problems or claim check;
  - research reports skip the quote and claim checks;
  - the outcome is `error` instead of `budget_exhausted`, and the usage log entry is lost.

The fix: every call that can block inside a run is limited to the time left before the deadline. A run that
reaches its deadline ends as `BudgetExhausted` with its full report. The watchdog stays, as the backstop.

WD2 builds on WD1 (it needs `Request.timeout`). Run them one after the other.

### Task WD1: a per-request timeout in every provider

Files: `crates/blyte-provider/src/{lib,gemini,anthropic,openai,ollama,chatgpt}.rs`; the places that build a `Request`
(`grep -rn "Request {" crates`), only to add the new field; new tests in `crates/blyte-provider/tests/`.

No fallbacks.

1. Add `pub timeout: Option<std::time::Duration>` to `Request` (`crates/blyte-provider/src/lib.rs`, about line 189),
   with a doc comment: the most this one call may take, from send to the last byte; `None` keeps each provider's own
   limits. Every struct literal that builds a `Request` gets `timeout: None`; do not change their behaviour.
2. In each provider's `complete`, when `req.timeout` is `Some(t)`, no limit may exceed `t`:
   - the connect, read (idle) and whole-call limits, each becoming `min(its current value, t)`;
   - Gemini's stream cap in `parse_stream` (`gemini.rs`, about lines 571-585 and 793-835).
   - If the call runs out of time, return the provider's existing timeout error (`ProviderError` kind as today for a
     timeout), with a message that says the request's time limit of `t` was reached.
   - Do not retry inside the provider after the limit.
3. Tests, one per provider, using the local test server pattern already in that provider's `*_wire.rs` test:
   - The server accepts the request and sends nothing for 30 s.
   - `complete` with `timeout: Some(1 s)` must return an error within 10 s.
   - A second case sends a normal reply quickly with `timeout: Some(10 s)` and succeeds.
   - Do not assert tight timings: only "returns well before the server's 30 s".

Gate: the repository checks. Commit: `feat(provider): let a request carry its own time limit`

### Task WD2: the runner stops at the deadline with its report (after WD1)

Files: `crates/blyte-agent/src/run.rs`, `crates/blyte-agent/src/tools.rs`, `crates/blyte-agent/src/claim_check.rs`;
the report fields in `crates/blyte-cli/src/report.rs`; the review output that shows the claim check
(`grep -rn "claim_check" crates/blyte-cli/src`). Grep first; never read `run.rs` or `dispatch.rs` whole.
New tests in `crates/blyte-agent/tests/`.

No fallbacks.

1. **Model calls** (`run_loop`, about lines 1018-1065): before `provider.complete`, compute the time left before
   `deadline`.
   - If none is left, return `Outcome::BudgetExhausted` with the report as it stands.
   - Otherwise set `req.timeout = Some(time_left)`.
   - In the `Err` arm, if the deadline has passed (or a retry wait would pass it, about line 1041), return
     `BudgetExhausted` instead of retrying or returning `Error`.
   - With no deadline, leave `req.timeout` as `None`.
2. **Bash** (`tools::execute`, about lines 614-639 and 731): the command timeout is
   `min(COMMAND_TIMEOUT, time left)` when there is a deadline. Pass the deadline in. Do not start a tool call once
   the deadline has passed; end the run as above.
3. **Gate** (`run_step_in`, about line 544): the gate's 1800 s timeout becomes `min(1800 s, time left)`. If the
   deadline passes during the gate, the run ends `BudgetExhausted`, and the report says the gate ran out of time.
4. **Claim check** (`run_claim_check`, about line 682, and `claim_check::check_claims`, `claim_check.rs:160`):
   - Each batch's request carries the time left.
   - When the deadline has passed, no new batch starts.
   - A claim check that could not run, or stopped part-way, because of the deadline must be visible, not a silent
     `None`: add a report field such as `claim_check_skipped: Option<String>` ("out of time after N of M batches").
     `blyte review` prints it as a `!` line under `== checks`.
   - The quote check (`check_report_quotes`, local, no model calls) still runs.
5. Tests (scripted providers and fake shells, as in `crates/blyte-agent/tests/test_time_left.rs`; no wall-clock
   races):
   - A provider that blocks until its `req.timeout` passes, then returns the timeout error, with a 2 s
     `max_duration`: the outcome is `BudgetExhausted`, `turn_usage` is not empty, and the provider saw
     `req.timeout <= 2 s`.
   - A fake shell that records the timeout it was given: with a deadline 5 s away, the bash timeout it got is
     `<= 5 s`; with no deadline it is `COMMAND_TIMEOUT`.
   - A research run whose deadline passes before the claim check: the report has `claim_check_skipped`, and
     `quote_problems` is present.
   - With no `max_duration`, requests carry `timeout: None` (no behaviour change).

Gate: the repository checks. Commit: `fix(agent): stop at the deadline with a full report instead of being killed by the watchdog`
