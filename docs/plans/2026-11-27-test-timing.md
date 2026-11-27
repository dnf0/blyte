# Tests that fail only on a busy machine: remaining tight time limits

The full suite runs test binaries in parallel, so any test can run many times slower than it
does alone. A research run (TTR1, at commit `2567cc8b`) read twelve suspected test files and
listed 23 more. Its report gives, for every finding, the lines, the quoted code and the smallest
correct fix. The report is not in the repository: it is in the main checkout at
`.wt/TTR1.report.md`, which from a run's worktree (`.wt/<ID>`) is `../TTR1.report.md`.

## Rules for every task

- Read your sections of `../TTR1.report.md` first. Line numbers are as of `2567cc8b`; check each
  finding against the code before changing it. If a finding is wrong, leave that code alone and
  say so in your final message.
- Apply each finding's "Smallest correct fix". Wait on the condition with a deadline of 30 s or
  more and a failure message that names what was awaited. Where a test's purpose is a timeout,
  keep the proof (the kill, the reported reason) and make the upper bound generous.
- No retries of a whole test, no `#[ignore]`, no serial running, no longer fixed sleeps standing
  in for a wait. No fallbacks: a wait that times out fails the test with a message; it never
  returns a partial result.
- Change only the files your task names. Do not change product code unless the task says so.
- There is no shared test helper module; do not create one. Each file keeps its own helpers.
- The other tasks of this plan run at the same time in other worktrees and are not in yours.
- Nothing here is a long-running job, so "resumable" does not apply.

### Task TT1

**Goal:** remove the tight limits in five blyte-agent test files.
**Depends on:** none. **Siblings:** TT2, TT3, TT4 (other files; not in this worktree).

**Files:** `crates/blyte-agent/tests/test_shell_timeout.rs`, `test_shell_background.rs`,
`test_blyte_tool.rs`, `test_gate_slot_deadline.rs`, `test_live_events.rs` (report sections 1, 7,
8, 9, 10), and `crates/blyte-agent/src/tools.rs` for one change only:

- `BACKGROUND_STARTUP_WAIT` stays 2 s by default. Add
  `pub fn set_background_startup_wait(wait: Option<Duration>)` beside `set_blyte_tool`, in the
  same style (`None` restores the default), and make `run_background_dispatch` use it. The tests
  for an early exit (BD1, BD3, BD5) set 30 s; the tests for "still running after the wait" set a
  short wait and use a gate file, as the report's finding 8.3 describes. If the setter is
  process-wide, tests in that file that set it must not race each other: guard them with the
  mechanism the file already uses for `set_blyte_tool`.

**Tests:** `cargo test -p blyte-agent --test test_shell_timeout --test test_shell_background --test test_blyte_tool --test test_gate_slot_deadline --test test_live_events`

**Commit message:** `test(agent): shell, background-dispatch, gate-slot and live-event tests wait on conditions, not short limits`

### Task TT2

**Goal:** remove the tight limits in the provider timing tests and two agent tests.
**Depends on:** none. **Siblings:** TT1, TT3, TT4.

**Files:** `crates/blyte-provider/tests/test_request_timeout.rs` (report section 3);
`crates/blyte-provider/tests/test_model_timeouts.rs` and `test_stream_timing.rs`,
`crates/blyte-agent/tests/test_provider_retry.rs` and `test_gate_slots_nesting.rs` (items 1, 3,
5 and 22 of the report's "Other Test Files" list; those have one line each, so read the test and
work out the smallest fix under the rules above). For a test that proves a stream survives
because data keeps arriving, widen the margin between the gap between chunks and the idle limit
(for example 20 ms chunks against an idle limit of several seconds) so a scheduler pause cannot
fail it. Do not change product code.

**Tests:** `cargo test -p blyte-provider --test test_request_timeout --test test_model_timeouts --test test_stream_timing && cargo test -p blyte-agent --test test_provider_retry --test test_gate_slots_nesting`

**Commit message:** `test(provider): timeout and stream-timing tests keep wide margins on a busy machine`

### Task TT3

**Goal:** remove the tight limits in six blyte-cli test files, and the 2 s limit on
`blyte statusline --after`.
**Depends on:** none. **Siblings:** TT1, TT2, TT4.

**Files:** `crates/blyte-cli/tests/test_http.rs`, `test_statusline.rs`, `test_one_word_prompt.rs`,
`test_no_gate_slot.rs`, `test_session.rs`, `test_requests.rs` (report sections 2, 4, 5, 6, 11,
12), and `crates/blyte-cli/src/statusline.rs` for one change only:

- The `--after` command's limit becomes a named constant of 5 s (a user's command that calls
  `git` on a busy machine could exceed 2 s). The environment variable
  `BLYTE_STATUSLINE_AFTER_TIMEOUT_MS`, when set, replaces it; a value that is not a whole number
  greater than zero is an error on stderr naming the variable and the value, with a non-zero
  exit (no fallback to the default). Document the variable where `--after` is documented
  (`grep -rn -- '--after' docs/ crates/blyte-cli/src/cli.rs`). The tests that run `--after`
  through the CLI set it to 30000. Add tests: the variable is honoured (a command sleeping 0.3 s
  with a limit of 100 ms is cut off; with 30000 it is not), and a bad value fails with the
  message.

**Tests:** `cargo test -p blyte --test test_http --test test_statusline --test test_one_word_prompt --test test_no_gate_slot --test test_session --test test_requests`

**Commit message:** `fix(statusline): the --after command gets 5 s, settable; cli tests wait on conditions, not short limits`

### Task TT4

**Goal:** remove the tight limits in the remaining blyte-cli test files.
**Depends on:** none. **Siblings:** TT1, TT2, TT3.

**Files (tests only, in `crates/blyte-cli/tests/`):** `test_surfaces_stopped.rs`,
`test_surface_liveness.rs`, `test_run_results.rs`, `test_session_peer.rs`,
`test_session_activity.rs`, `test_clean_surfaces.rs`, `test_surface_dir.rs`,
`test_surface_notify.rs`, `test_surface_registry.rs`, `test_surface_switcher.rs`,
`test_surfaces_cmd.rs`, `test_dispatch_base_check.rs`, `test_passed_trees.rs`,
`test_clean_followups.rs`, `test_collect_fast.rs`, `test_deadline_watchdog.rs`,
`test_surface_resume_memory.rs`, `test_surface_stop_line.rs`, `test_dispatch_request.rs`
(items 2, 4, 6 to 21 and 23 of the report's "Other Test Files" list; one line each, so read each
test and work out the smallest fix under the rules above). Typical fixes: a mock server's read
limit of 200 ms or 2 s becomes 30 s; a 5 s or 15 s wait becomes 30 s and fails with a message
instead of returning a partial result; a model limit shorter than the wait it sits inside is
raised above it; an `elapsed < 10 s` bound on a subprocess is raised to 60 s unless the test's
purpose is speed, in which case say so in a comment and use 60 s. Where a test's mock answers
slower than a product limit on purpose (`test_surface_liveness.rs` against
`PROBE_RETRY_TIMEOUT`), keep the purpose and widen the margin on the test's side only. Do not
change product code; if a test cannot be made safe without a product change, leave it and say so
in your final message.

**Tests:** run each changed file: `cargo test -p blyte --test <name>` for every file above.

**Commit message:** `test(cli): mock servers and waits in surface, clean and dispatch tests allow 30 s`
