# A fast test suite

Date: 2026-11-23

## Goal

`cargo test --workspace --all-targets` spends 300 s running tests on a warm build (measured
2026-11-23 after #407; the sum of each test file's `finished in` time over 250 files). 63 files
take more than 1 s and sum to 264 s. The goal is under 100 s for the whole suite, with every test
and every assertion kept.

Six tasks, each a batch of slow test files, all independent (different files). A last task, written
after these land, makes the test files safe to run side by side.

## Method

Every task follows this method.

1. Run each file alone and time each test: `cargo test -p <package> --test <file> -- --test-threads=1 --nocapture`.
   Read the code path each slow test drives and find where the time goes before changing anything.
2. Causes found so far in this repository (2026-11-23, #407), to look for first:
   - A shell loop that starts hundreds of small processes (`$(seq ...)`, `$(printf ...)` inside a loop).
   - A real shell (`LocalShell`), a real git repository or a real `Session` in a test that only
     checks turn-loop or bookkeeping behaviour. Use an in-process stand-in only where the test
     asserts nothing about what the shell or git does.
   - `blyte` started as a child process without `BLYTE_NO_UPDATE_CHECK=1`, so each start makes a
     real release check against GitHub (3 s timeout).
   - Child `blyte dispatch` processes queueing for the shared check slot: give the test its own
     `BLYTE_GATE_SLOT_DIR` (a directory inside the test's temporary directory).
   - A fixed `sleep` before asserting that nothing happened: assert the state directly instead
     (queue empty, not busy), if the code exposes it.
   - A poll loop with a long interval, or a timeout waited out in full.
3. Remove the cause:
   - A wait in the test itself: shorten or remove it.
   - A fixed wait or interval in production code: do not change the production value. Make it a
     field of the configuration struct that code already takes, with the present value as its
     default, and have the test set a short value.
   - A test that needs its time because it measures a real timeout: shorten the timeout it
     configures if the test controls it; otherwise leave it and say so.

Rules for every task:

- Every existing test and assertion stays. Do not delete, `#[ignore]`, merge or weaken a test.
- Production behaviour does not change. No new command-line flag, `blyte.toml` key or environment
  variable.
- The only change allowed under `src/` is making an existing wait, interval or clock injectable
  (a parameter or a configuration field whose default is the present value). Do not change how
  production code starts processes, reads or writes files, makes temporary directories or calls
  git, even when the change looks equivalent and faster: report it as a finding instead. (Added
  after run FSA1 rewrote session temporary directories and replaced `cat` and `git rev-parse` in
  product code; those changes were removed in review.)
- No fallbacks: do not replace a failing wait with a default or a skipped check.
- Do not make a test depend on timing more than it does now: a test that passes alone must also
  pass while the machine is busy.
- Change only the files the task names, plus production source only as rule 3 allows. If the cause
  is in a file another task owns or in a helper shared by files outside this task, leave it and
  report it.
- Target: each file under 1.5 s run alone. A file that misses it needs a reason in the report.
- Report, for each file: time before, time after (run alone), the cause, the change. Describe only
  changes that are in the final diff.

### Task C1: make these blyte test files fast

**Depends on:** none.

**Siblings:** tasks C2, C3, A1, A2, P1 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. In particular do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it. Times are from
one full run of the suite on 2026-11-23.

| File | Now |
|---|---|
| `crates/blyte-cli/tests/test_fix_round.rs` | 14.7 s |
| `crates/blyte-cli/tests/test_dispatch_base_check.rs` | 10.1 s |
| `crates/blyte-cli/tests/test_clean_sessions.rs` | 9.6 s |
| `crates/blyte-cli/tests/test_clean.rs` | 6.5 s |
| `crates/blyte-cli/tests/test_dispatch_branch_exists.rs` | 5.5 s |
| `crates/blyte-cli/tests/test_dispatch_start_first.rs` | 3.4 s |
| `crates/blyte-cli/tests/test_map_rust.rs` | 3.1 s |
| `crates/blyte-cli/tests/test_orchestrator_sandbox.rs` | 2.6 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Run a file with `cargo test -p blyte --test <file>`.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files (plus production source only as the Method allows).

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact):**

```
test: make slow blyte test files fast (C1)
```

### Task C2: make these blyte test files fast

**Depends on:** none.

**Siblings:** tasks C1, C3, A1, A2, P1 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. In particular do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it. Times are from
one full run of the suite on 2026-11-23.

| File | Now |
|---|---|
| `crates/blyte-cli/tests/test_clean_followups.rs` | 11.7 s |
| `crates/blyte-cli/tests/test_serve.rs` | 10.0 s |
| `crates/blyte-cli/tests/test_requests.rs` | 8.8 s |
| `crates/blyte-cli/tests/test_live_runs.rs` | 6.4 s |
| `crates/blyte-cli/tests/test_dispatch.rs` | 4.9 s |
| `crates/blyte-cli/tests/test_clean_quiet_squash.rs` | 3.4 s |
| `crates/blyte-cli/tests/test_fresh_model_review.rs` | 2.8 s |
| `crates/blyte-cli/tests/test_clean_background.rs` | 2.5 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Run a file with `cargo test -p blyte --test <file>`.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files (plus production source only as the Method allows).

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact):**

```
test: make slow blyte test files fast (C2)
```

### Task C3: make these blyte test files fast

**Depends on:** none.

**Siblings:** tasks C1, C2, A1, A2, P1 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. In particular do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it. Times are from
one full run of the suite on 2026-11-23.

| File | Now |
|---|---|
| `crates/blyte-cli/tests/test_usage_session.rs` | 10.6 s |
| `crates/blyte-cli/tests/test_publish.rs` | 9.7 s |
| `crates/blyte-cli/tests/test_blyte_toml.rs` | 8.4 s |
| `crates/blyte-cli/tests/test_root_main_checkout.rs` | 6.0 s |
| `crates/blyte-cli/tests/test_watchdog_progress.rs` | 4.4 s |
| `crates/blyte-cli/tests/test_deadline_watchdog.rs` | 3.4 s |
| `crates/blyte-cli/tests/test_statusline.rs` | 2.7 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Run a file with `cargo test -p blyte --test <file>`.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files (plus production source only as the Method allows).

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact):**

```
test: make slow blyte test files fast (C3)
```

### Task A1: make these blyte-agent test files fast

**Depends on:** none.

**Siblings:** tasks C1, C2, C3, A2, P1 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. In particular do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it. Times are from
one full run of the suite on 2026-11-23.

| File | Now |
|---|---|
| `crates/blyte-agent/tests/test_max_cost.rs` | 7.5 s |
| `crates/blyte-agent/tests/test_turns_left.rs` | 6.5 s |
| `crates/blyte-agent/tests/test_run.rs` | 4.8 s |
| `crates/blyte-agent/tests/test_session_protocol.rs` | 4.1 s |
| `crates/blyte-agent/tests/test_persistent_shell.rs` | 3.6 s |
| `crates/blyte-agent/tests/test_blyte_tool.rs` | 3.3 s |
| `crates/blyte-agent/tests/test_research_wrap_up.rs` | 3.0 s |
| `crates/blyte-agent/tests/test_turn_stream_timing.rs` | 2.3 s |
| `crates/blyte-agent/tests/test_truncated_reply.rs` | 2.2 s |
| `crates/blyte-agent/tests/test_read_loop.rs` | 2.1 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Run a file with `cargo test -p blyte-agent --test <file>`.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files (plus production source only as the Method allows).

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact):**

```
test: make slow blyte-agent test files fast (A1)
```

### Task A2: make these blyte-agent test files fast

**Depends on:** none.

**Siblings:** tasks C1, C2, C3, A1, P1 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. In particular do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it. Times are from
one full run of the suite on 2026-11-23.

| File | Now |
|---|---|
| `crates/blyte-agent/tests/test_model_timeout_transport.rs` | 7.4 s |
| `crates/blyte-agent/tests/test_time_left.rs` | 5.0 s |
| `crates/blyte-agent/tests/test_network_off.rs` | 4.7 s |
| `crates/blyte-agent/tests/test_blocked.rs` | 3.6 s |
| `crates/blyte-agent/tests/test_provider_retry.rs` | 3.5 s |
| `crates/blyte-agent/tests/test_shell_background.rs` | 3.1 s |
| `crates/blyte-agent/tests/test_approval.rs` | 2.4 s |
| `crates/blyte-agent/tests/test_gate_slot_deadline.rs` | 2.3 s |
| `crates/blyte-agent/tests/test_quota_exhausted.rs` | 2.2 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Run a file with `cargo test -p blyte-agent --test <file>`.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files (plus production source only as the Method allows).

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact):**

```
test: make slow blyte-agent test files fast (A2)
```

### Task P1: make these blyte-provider test files fast

**Depends on:** none.

**Siblings:** tasks C1, C2, C3, A1, A2 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. In particular do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it. Times are from
one full run of the suite on 2026-11-23.

| File | Now |
|---|---|
| `crates/blyte-provider/tests/test_request_timeout.rs` | 8.0 s |
| `crates/blyte-provider/tests/test_model_timeouts.rs` | 3.5 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Run a file with `cargo test -p blyte-provider --test <file>`.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files (plus production source only as the Method allows).

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact):**

```
test: make slow blyte-provider test files fast (P1)
```

## Second round (2026-11-24)

After the first round the suite's test time is about 165 s (from 300 s). These files are still slow.
The machine runs a virus scanner that slows every process a test starts, so times vary between runs:
the fewer processes and git commands a test starts, the faster and steadier it is.

### Task D1: make these blyte test files fast (second round)

**Depends on:** none.

**Siblings:** tasks D2, D3 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. Do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it.

| File | Now |
|---|---|
| `crates/blyte-cli/tests/test_fix_round.rs` | 7-15 s |
| `crates/blyte-cli/tests/test_fresh_model_review.rs` | 4-13 s |
| `crates/blyte-cli/tests/test_direct_edit.rs` | 6.8 s |
| `crates/blyte-cli/tests/test_dispatch_request.rs` | 5.2 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Start with the slowest file and finish and commit one file before starting the
next, so that nothing is left uncommitted if the budget runs out. Run a file with
`cargo test -p blyte --test <file>`. If a file cannot get under 1.5 s without breaking a rule
(for example because the test has to start several real `blyte` processes), stop on that file and
say in the report what the remaining time is spent on.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files. Under `src/` the only change allowed is making an
existing wait, interval or clock settable with its present value as the default; anything else you
find there is a finding for the report, not a change.

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact, one commit per file is fine, each with this message):**

```
test: make slow blyte test files fast (D1)
```

### Task D2: make these blyte test files fast (second round)

**Depends on:** none.

**Siblings:** tasks D1, D3 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. Do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it.

| File | Now |
|---|---|
| `crates/blyte-cli/tests/test_clean_followups.rs` | 5-17 s |
| `crates/blyte-cli/tests/test_clean_quiet_squash.rs` | 8.2 s |
| `crates/blyte-cli/tests/test_map_rust.rs` | 4.3 s |
| `crates/blyte-cli/tests/test_requests.rs` | 3.9 s |
| `crates/blyte-cli/tests/test_passed_trees.rs` | 2.5-5.6 s |
| `crates/blyte-cli/tests/test_dispatch_base_check.rs` | 2.4-7.4 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Start with the slowest file and finish and commit one file before starting the
next, so that nothing is left uncommitted if the budget runs out. Run a file with
`cargo test -p blyte --test <file>`. If a file cannot get under 1.5 s without breaking a rule
(for example because the test has to start several real `blyte` processes), stop on that file and
say in the report what the remaining time is spent on.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files. Under `src/` the only change allowed is making an
existing wait, interval or clock settable with its present value as the default; anything else you
find there is a finding for the report, not a change.

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact, one commit per file is fine, each with this message):**

```
test: make slow blyte test files fast (D2)
```

### Task D3: make these blyte-agent test files fast (second round)

**Depends on:** none.

**Siblings:** tasks D1, D2 do the same for other test files, separately. Their work is not in this
worktree; do not change their files. Do not touch any test file not listed here.

**Goal.** Make each test file below run in under 1.5 s alone, without weakening it.

| File | Now |
|---|---|
| `crates/blyte-agent/tests/test_run_timing.rs` | 5.7 s |
| `crates/blyte-agent/tests/test_session_protocol.rs` | 3.9 s |
| `crates/blyte-agent/tests/test_stay_in_repo.rs` | 3.0 s |
| `crates/blyte-agent/tests/test_tool_event_input.rs` | 2.7 s |
| `crates/blyte-agent/tests/test_live_events.rs` | 2.6 s |
| `crates/blyte-agent/tests/test_shell_timeout.rs` | 2.1 s |

**How.** Read the section "Method" of `docs/plans/2026-11-23-fast-suite.md` first and follow it,
including its rules. Start with the slowest file and finish and commit one file before starting the
next, so that nothing is left uncommitted if the budget runs out. Run a file with
`cargo test -p blyte-agent --test <file>`. If a file cannot get under 1.5 s without breaking a rule
(for example because the test has to start several real `blyte` processes), stop on that file and
say in the report what the remaining time is spent on.

**Constraints.** As the Method's rules: every test and assertion kept, production behaviour
unchanged, no fallbacks, only the listed files. Under `src/` the only change allowed is making an
existing wait, interval or clock settable with its present value as the default; anything else you
find there is a finding for the report, not a change.

**Tests.** The listed files themselves, each run alone, with its `finished in` time in your report
next to the cause and the change.

**Commit message (exact, one commit per file is fine, each with this message):**

```
test: make slow blyte-agent test files fast (D3)
```
