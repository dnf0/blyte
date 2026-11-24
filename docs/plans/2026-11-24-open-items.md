# Open items: internal commands out of the help, and the items that need facts first

Date: 2026-11-24. One pull request for all tasks. Tasks for the surface starting without a
terminal and the Review count are added once research run RS4 has checked the facts. Task FR
rests on research run RS5 (`.wt/RS5.report.md`).

## Rules for every task

- No fallbacks: when something is missing or does not match, fail with an error that names it.
- Existing tests pass unmodified unless the task says otherwise.
- Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings`, then the tests the
  task names.
- No new dependencies.

### Task HELP

**Goal.** `blyte --help` lists the commands a person or an orchestrating model uses. The five
commands that only blyte itself calls still work but are not listed.

**Depends on:** none.

**Siblings.** Later tasks in this plan change `serve.rs`, `repl.rs`, `main.rs`, `dispatch.rs` and
`queue_actions.rs`. Do not touch those files.

**Files.** `crates/blyte-cli/src/cli.rs`, new `crates/blyte-cli/tests/test_help_commands.rs`,
`docs/reference.md` only if it has a list of commands that needs a note.

**What to build.** In the subcommand enum in `cli.rs`, add `#[command(hide = true)]` to exactly
these variants: `Comments`, `Instructions`, `Mock`, `EditStart`, `EditFinish`. Nothing else
changes: the commands parse and run as before, and `blyte <command> --help` still prints their
own help. Do not hide, rename or remove any other command. If the top-level help text has an
`after_help` or a hand-written command list that names any of the five, remove them from that
list too.

**Tests** (`test_help_commands.rs`, running the built binary as the other CLI tests do, for
example `test_cli_smoke.rs`).

- `the_help_does_not_list_internal_commands`: in the output of `blyte --help`, no line of the
  commands list starts (after leading spaces) with `comments`, `instructions`, `mock`,
  `edit-start` or `edit-finish`.
- `the_help_still_lists_the_commands_people_use`: the commands list has lines starting with
  `dispatch`, `review`, `wait`, `status`, `plan`, `cost`, `clean`, `init` and `update`. First check
  each of these exists in the enum; if one does not, leave it out of the test and say so in your
  report, do not add a command.
- `hidden_commands_still_run`: `blyte edit-start --help`, `blyte edit-finish --help`,
  `blyte comments --help`, `blyte instructions --help` and `blyte mock --help` each exit 0 and
  print a usage line.

Run: `cargo test -p blyte --test test_help_commands --test test_cli_smoke`.

**Commit message.** `feat(cli): leave blyte's internal commands out of the help`

### Task FR

**Goal.** When the model review of a dispatched run raises concerns, the run's one fix round is
given them, as it is already given confirmed fallbacks. Today a dispatched run gets a fix round
only for confirmed fallbacks (`fix_round::should_run` tests `confirmed_count(mr) > 0`), and the
review's `concerns` are only shown to the reviewer. A direct edit (`direct_edit.rs`) already
sends both; this makes a dispatched run do the same.

**Depends on:** none.

**Siblings.** Task HELP changes `cli.rs`; later tasks change `serve.rs`, `repl.rs`, `main.rs` and
`assets/surface/index.html`. They are not in this worktree; do not touch those files.

**Files.** `crates/blyte-cli/src/fix_round.rs`, `crates/blyte-cli/src/dispatch.rs`,
`crates/blyte-cli/src/direct_edit.rs`, `crates/blyte-cli/src/review_checks.rs`,
`crates/blyte-cli/src/config.rs` (doc comment only), `crates/blyte-cli/tests/test_fix_round.rs`,
`crates/blyte-cli/tests/test_fresh_model_review.rs`, and `docs/reference.md` where it describes
the fix round.

**What to build.**

1. `fix_round::should_run`: after the existing early returns, return
   `confirmed_count(mr) > 0 || !mr.concerns.is_empty()`. A failed review, a research run, a run
   that is not `verified` and `fix_round = false` still give `false`.
2. `FixRoundRecord::Started` and `FixRoundRecord::Done` each get
   `#[serde(default)] concerns_sent: Vec<crate::model_review::Concern>`, so a `.fixround.json`
   written before this change still loads (with no concerns). `FixRoundSummary` gets
   `#[serde(default)] concerns_left: Option<usize>`: the number of concerns in the review that
   runs after the round; `None` when no review ran or it failed (the same rule as
   `fallbacks_left`; add `fix_round::concerns_left(review: Option<&ModelReviewResult>)`).
3. `fix_round::task_text(original_task, fallbacks, concerns)` takes `&[Concern]` as a third
   argument.
   - With no concerns it returns exactly the text it returns today (the existing unit test's
     expected text must still match, with the new argument passed as `&[]`).
   - With concerns and no fallbacks the preamble is:
     `This work passed its check, and a review raised the concerns listed below.` /
     `For each one, change the code or the tests so that the concern no longer holds.` /
     `If a listed concern is wrong, leave the code as it is and say why in the final message.` /
     `Change nothing else.` / ``Commit with the message `fix: address the review's concerns`.``
     then a blank line and one line per concern, `[<severity>] <file>:<line> <text>`, or
     `[<severity>] <file> <text>` when the line is `None`, in the order given.
   - With both, the text starts with today's fallback preamble and fallback lines, except that
     its commit line names `fix: address the review's concerns`, then a blank line, the line
     `The review also raised these concerns. For each one, change the code or the tests so that the concern no longer holds; if one is wrong, leave the code as it is and say why in the final message.`
     and the concern lines.
   - The `## Original task (for context; already done)` section follows in every case, as today.
4. New `pub const CONCERNS_COMMIT_MESSAGE: &str = "fix: address the review's concerns";` in
   `fix_round.rs`. `COMMIT_MESSAGE` keeps its value and is still the message for a round with
   only fallbacks. In `review_checks.rs`, everywhere `crate::fix_round::COMMIT_MESSAGE` is
   accepted or listed as an expected commit message (lines 188 and 244 today), accept or list
   `CONCERNS_COMMIT_MESSAGE` in the same way.
5. `dispatch.rs`:
   - in the block that starts a fresh fix round (after `should_run` is true), take
     `concerns_sent = review.concerns.clone()`; call `collect_confirmed_fallbacks` only when
     `confirmed_count(review) > 0`, else use an empty list (do not read `.checks.json` for
     fallbacks that do not exist); write both into `FixRoundRecord::Started`;
   - in the block that resumes a round found `Started` after a crash, take `concerns_sent` from
     the record, so the resumed round sends what the first attempt sent (resumable);
   - `execute_fix_round` takes the concerns, passes them to `task_text`, writes them into
     `FixRoundRecord::Done`, and sets `concerns_left` from the review that runs after the round.
   The round is still the only one: concerns left after it stay as flags on the run for the
   reviewer, exactly as confirmed fallbacks left after it do today. The rollback rule is
   unchanged (a round that does not pass, or makes no commit, is reset to `pre_round_sha`).
6. `direct_edit.rs`: its behaviour does not change. Fill `concerns_sent` in the
   `FixRoundRecord::Started` it writes from `mr.concerns.clone()`, and reduce `needs_fix_round`
   to the `should_run` call alone, since `should_run` now covers concerns.
7. `config.rs`: the doc comment on `DispatchConfig::fix_round` says the round runs "when the
   fresh review confirms fallbacks or raises concerns".

No fallbacks: a `.fixround.json` that cannot be parsed is an error naming the file, as today; do
not treat an unreadable record as "no concerns".

**Tests.**

`test_fix_round.rs`:
- update `unit_task_text_and_should_run_and_config` for the new `task_text` argument; its
  expected text for fallbacks only is unchanged;
- `should_run_is_true_for_a_review_with_only_concerns`: a succeeded review with one concern and
  no fallbacks gives `true`; the same review with `fix_round_enabled = false`, with
  `is_research = true`, or with outcome `gate_failed` gives `false`;
- `task_text_lists_concerns_alone`: two concerns (one with a line, one with `None`) and no
  fallbacks give the preamble above, the two lines in order in the stated forms, the concerns
  commit message, and the original task section; the text does not contain the word `fallback`;
- `task_text_lists_fallbacks_then_concerns`: one of each gives the fallback line before the
  concern line, and names `fix: address the review's concerns` and not
  `fix: fail loudly where review found a fallback`;
- `an_old_fixround_record_loads_without_concerns`: a `Started` and a `Done` record written as
  JSON without `concerns_sent` / `concerns_left` parse, with an empty list and `None`;
- `a_review_with_only_concerns_starts_a_fix_round`: an end-to-end dispatch in the style of the
  file's existing end-to-end tests (same mock server). The first review returns one `high`
  concern and `"fallbacks": []`; assert a second implementer pass ran, the text it was sent
  contains the concern's `[high] <file>:<line> <text>` line, `.wt/<id>.fixround.json` is `done`
  with that concern in `concerns_sent`, and the summary's `fix_round.concerns_left` equals the
  number of concerns the second review returned (make the second review return none, so 0);
- `a_commit_with_the_concerns_message_is_not_flagged`: after that round, the run's checks do not
  flag the commit `fix: address the review's concerns` as an unexpected commit message.

`test_fresh_model_review.rs`: `flagged_run_triggers_fresh_review_and_findings_wait_in_queue` and
`risky_path_triggers_review_and_findings_put_run_in_queue` assume a review with only concerns
runs no fix round. Set `fix_round = false` under `[dispatch]` in the `blyte.toml` those two tests
write, and change nothing else in them: what they pin (the review runs, its findings put the run
in the queue) is not about the fix round.

Run: `cargo test -p blyte --test test_fix_round --test test_fresh_model_review --test test_direct_edit --test test_review_checks`
(leave out a name that is not a test file in `crates/blyte-cli/tests/`, and say so).

**Commit message.** `feat(dispatch): send the review's concerns back in the fix round`

### Task TERM

**Goal.** Bare `blyte` starts the surface whether or not its standard input is a terminal, so it
can be started by a service manager or with `< /dev/null`. Today `main.rs` sends any start whose
input is not a terminal to the old terminal session, which reads end of input and exits 0 without
a word (research RS4, `.wt/RS4.report.md`). The surface itself never reads standard input and
already runs until SIGINT or SIGTERM. After this change the terminal session is reached only
with `--terminal`.

**Depends on:** none.

**Siblings.** Task RC changes `assets/surface/index.html`, `dispatch.rs` and
`tests/test_surface_page.rs`; it is not in this worktree. Do not touch those files, `serve.rs` or
`repl.rs`.

**Files.** `crates/blyte-cli/src/main.rs`, `crates/blyte-cli/tests/test_serve.rs`,
`crates/blyte-cli/tests/test_e2e_cli.rs`, any other test file that starts bare `blyte` with piped
input and expects the terminal session (find them: `grep -rn "Stdio::piped" crates/blyte-cli/tests`),
`docs/usage.md` and `docs/reference/cli.md` where they describe how `blyte` starts, and the
`--terminal` help text in `cli.rs` only if it says the terminal session is chosen by piped input.

**What to build.**

1. In `main.rs`, `let use_terminal = cli.terminal || !std::io::stdin().is_terminal();` becomes
   `let use_terminal = cli.terminal;`. Remove the `IsTerminal` import if nothing else uses it.
   Nothing else in the start path changes: no check of what standard input is, no new flag.
2. Tests that drive the terminal session through piped input pass `--terminal`:
   `test_cli_interactive_repl_stdin_driving` and `the_banner_is_one_line_with_the_name` in
   `test_e2e_cli.rs`, and any other you find. Their assertions do not change.
3. In `test_serve.rs`, replace `piped_stdin_still_runs_the_terminal_session` with the two tests
   below.

No fallbacks: do not add a path that falls back to the terminal session when the surface cannot
start; a surface that cannot start prints its error and exits non-zero, as it does today.

**Tests** (`test_serve.rs`, running the built binary).

- `the_terminal_flag_runs_the_terminal_session_on_piped_input`: `blyte --terminal` with piped
  input `/exit\n` prints `Blyte Interactive Harness` and exits 0 (the old test with the flag
  added).
- `bare_blyte_starts_the_surface_without_a_terminal` (`#[cfg(unix)]`): in a temporary git
  repository set up the way this file's other tests set one up (same environment variables, so
  no real model is called and no real home directory is read), spawn `blyte --no-open --port 0`
  with `Stdio::null()` as standard input and piped standard output. Read standard output lines
  until one starts with `blyte: surface at ` (fail the test, killing the child, if none arrives
  within 30 s or the child exits first, printing its standard error). Assert `.wt/surface.json`
  exists. Send SIGTERM with `kill -TERM <pid>` (`std::process::Command`; no new dependency).
  Assert the child exits 0 within 10 s and `.wt/surface.json` is gone.

Run: `cargo test -p blyte --test test_serve --test test_e2e_cli --test test_cli_smoke`.

**Commit message.** `feat(surface): start the surface when standard input is not a terminal`

### Task RC

**Goal.** The `Review (N)` number on the page stops going stale. Research RS4 found the normal
refresh is sound (the runs list is fetched every 5 s) and three faults:

1. after any API request gets a 403, the page stops all polling for good; when a new `#token=`
   then arrives in the address (the `hashchange` listener in `init()`), the token is taken but
   polling never restarts, so the count is frozen until a reload;
2. when `GET /api/runs` fails for another reason, the error is written to `#queue-error`, which
   is inside `#review-wrap` and so hidden on the Design tab: the count is stale with nothing
   said;
3. `blyte review <id> --verdict ...` does not tell a running surface, so the count is up to 5 s
   behind (and longer for anything that waits on the surface's own timer).

**Depends on:** none (the branch already has Task FR's change to `dispatch.rs`).

**Siblings.** Task TERM changes `main.rs`, `tests/test_serve.rs` and `tests/test_e2e_cli.rs`; it
is not in this worktree. Do not touch those files.

**Files.** `crates/blyte-cli/assets/surface/index.html`, `crates/blyte-cli/tests/test_surface_page.rs`,
`crates/blyte-cli/src/dispatch.rs` (`run_review` only), new
`crates/blyte-cli/tests/test_review_notify.rs`.

**What to build.**

1. `index.html`: move the three lines of `init()` that start polling
   (`runsPollTimer = setInterval(fetchRuns, RUNS_POLL_MS)`, the same for `fetchRequests`, and
   `pollEvents()`) and the first fetches that precede them into one function `startPolling()`
   that sets `pollingStopped = false`, and call it from `init()` where those lines were. In the
   `hashchange` listener, when a token is read and `pollingStopped` is true, call
   `startPolling()` after `updateMissingTokenUI()`. `startPolling()` must not leave two timers
   or two `pollEvents()` loops running: it does nothing for a timer that is not `null`, and
   `pollEvents()` must already end when `pollingStopped` is true (check it does; if it does not
   return on that flag, make it).
2. `index.html`: when `fetchRuns()` fails (not 403), show the failure where it can be seen from
   any tab: the header counter reads `Review (N) !` (the last known N, then a space and `!`) and
   has the error text as its `title`; the next successful `fetchRuns()` removes both. Keep
   writing the error to `#queue-error` as now. Use the page's existing way of setting text
   (no `innerHTML` with server text: `no_raw_html_from_the_server` must still pass).
3. `dispatch.rs` `run_review`: after `record_verdict` succeeds and before it prints
   `recorded verdict ...`, call `crate::surface_notify::notify_runs_changed(&root, &args.id)` and
   pass the outcome to `crate::surface_notify::report` exactly as `finish_summary` in the same
   file does. No surface running is not an error (that is what `NotifyOutcome::NoSurface` is
   for); a surface that is there and cannot be told is reported by `report`, and the verdict
   stays recorded and the exit code stays 0.

No fallbacks: do not swallow a fetch error to keep the old number looking current.

**Tests.**

`test_surface_page.rs`, with the file's `run_surface_script` harness (node):
- `polling_restarts_when_a_token_arrives_after_a_403`: with a stub `fetch` that returns 403 for
  `/api/runs`, run `init()`, check polling stopped (`runsPollTimer === null`); then set a hash
  with a token, make the stub return 200 with one run whose `verdict` is `null`, fire the
  `hashchange` listener, and check `runsPollTimer !== null`, that `/api/runs` was fetched again,
  and that the header counter reads `Review (1)`.
- `starting_polling_twice_leaves_one_timer`: call `startPolling()` twice; `setInterval` (stubbed
  to count) was called once per timer, and one `pollEvents` loop is running (count the event
  fetches).
- `a_failed_runs_fetch_marks_the_review_count`: after a good fetch with two undecided runs, the
  stub returns 500 with a text body; after `fetchRuns()` the counter text is `Review (2) !` and
  its `title` contains the error; after the next good fetch it is `Review (2)` with no `title`.
If the harness cannot reach one of these functions as written, expose it the way the file's
other script tests reach page functions; do not weaken an assertion.

`test_review_notify.rs`, modelled on `tests/test_dispatch_notify.rs` (same stand-in listener and
`.wt/surface.json` / `.wt/surface.token` setup):
- `recording_a_verdict_tells_a_running_surface`: with a stand-in surface listening,
  `blyte review <id> --verdict accepted` exits 0, prints `recorded verdict accepted for <id>`,
  and the listener got one `POST /api/runs/changed` with the token;
- `recording_a_verdict_without_a_surface_says_nothing_extra`: with no `.wt/surface.json`, the
  command exits 0 and standard error is empty;
- `a_surface_that_cannot_be_told_does_not_undo_the_verdict`: `.wt/surface.json` names a port
  nothing listens on; the command exits 0, the verdict is in the run's files, and standard error
  has the message `report` prints.

Run: `cargo test -p blyte --test test_surface_page --test test_review_notify --test test_dispatch_notify --test test_review_reasons`.

**Commit message.** `fix(surface): keep the Review count current after a 403, a failed fetch and a recorded verdict`
