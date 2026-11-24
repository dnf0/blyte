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
