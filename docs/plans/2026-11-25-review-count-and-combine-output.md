# Review count and failed-combine output

Two defects in the surface, found in use and confirmed by research run RS7 (2026-11-25, `main` at 800abdb4).

## Decisions

| Question | Decision |
|---|---|
| When does a combined run leave Review? | When its request is combined and current, the run is clean (`card == "clean"`) and it has no hold. A flagged run or a held run stays in Review until it has a verdict. |
| Where is "combined" decided? | On the server, by the same test the Requests view uses, so the two views cannot disagree. The page only reads a boolean. |
| What does a failed combine show? | The same cut of the failing step's output that a run's own check failure shows (`blyte_agent::run::cut_gate_output`), for every failing step. |
| Missing fields in a failure record | An error naming the field and the record file. No placeholder text. |

Not in this plan (noted for later): `requests.rs` uses the current runs when a combine exits 0 without a record; a `waiting` request status carries failure fields from an earlier combine; `setup.rs` keeps the last 20 lines of a failed setup (the full log is on disk).

### Task RV: combined runs leave the Review count and list

**Goal.** A run whose work the surface has already combined is no longer counted in `Review (N)` or listed in the Review view, unless it is flagged or held. Today the page counts every finished run with no verdict (`isUndecided`, `crates/blyte-cli/assets/surface/index.html`), and combining writes no verdict, so the count reads higher than what needs the user.

**Files.** `crates/blyte-cli/src/requests.rs`, `crates/blyte-cli/src/serve.rs`, `crates/blyte-cli/assets/surface/index.html`, `crates/blyte-cli/tests/test_serve.rs`, `crates/blyte-cli/tests/test_surface_page.rs`, and any other test or fixture that builds a `/api/runs` payload.

**What to build.**

1. `requests.rs`: `pub fn run_is_combined(root: &Path, run_id: &str, request: Option<&str>) -> Result<bool, String>`.
   - `request` is `None`: `Ok(false)`.
   - `load_combine_record(root, request)` returns `Ok(None)`: `Ok(false)`. It returns `Err`: return that error (it must name the record file; if `load_combine_record`'s error does not, add the path).
   - The record's `state` is not `CombineState::Combined`: `Ok(false)`.
   - The record's `runs` has no entry with this `run_id`: `Ok(false)`.
   - The record is not current, by the same test `build_request_status` uses (`record_runs_match_tips(&rec, &resolve_current_run_tips(root, request)?)`): `Ok(false)`. An error from `resolve_current_run_tips` is returned, not swallowed.
   - Otherwise `Ok(true)`.
2. `serve.rs` `handle_runs`: add `combined: bool` to `RunWithHold`, set from `run_is_combined(&surface.root, &run.id, run.request.as_deref())`. On `Err`, respond 500 with the error, as the hold read beside it does. Load each request's record and current tips once per response, not once per run (group runs by request).
3. `index.html`:
   - `validateRuns`: add `"combined"` to `reqBool`, so a payload without it is a validation error.
   - `isUndecided(run)`: `run.verdict === null && !(run.combined && run.card === "clean" && run.hold === null)`.
   - `isHeld(run)`: `isUndecided(run) && run.hold !== null`.
   - Nothing else changes: the count, the list, the group count and the maps all derive from `isUndecided`. Confirm that with a grep for `verdict === null` and change any other use to call `isUndecided`.

No fallbacks: a missing or non-boolean `combined` is an error on the page; an unreadable combine record is a 500 that names the file.

**Tests.**

- `test_serve.rs`:
  - `a_combined_run_without_a_verdict_is_marked_combined`: a verified run in a request, combined through the real path; `GET /api/runs` has `combined: true` and `verdict: null` for it.
  - `a_run_whose_request_is_not_combined_is_not_marked`: verified run, no combine record: `combined: false`.
  - `a_run_changed_after_the_combine_is_not_marked`: after the combine, add a commit to the run's branch: `combined: false`.
  - `a_run_in_a_failed_combine_is_not_marked`: record with state `check_failed`: `combined: false`.
  - `an_unreadable_combine_record_is_a_500_naming_the_file`: write invalid JSON to `.wt/combined/<request>.json`: status 500 and the body contains that path.
- `test_surface_page.rs` (the node harness already there):
  - `a_clean_combined_run_is_not_in_the_review_count_or_list`: two runs with `verdict: null`, one `combined: true, card: "clean", hold: null`: the tab reads `Review (1)` and the Review list has only the other run.
  - `a_flagged_combined_run_stays_in_review`: `combined: true, card: "exception"`: counted and listed.
  - `a_held_combined_run_stays_in_review`: `combined: true`, `hold` set: counted, listed under held.
  - `a_runs_payload_without_combined_is_an_error`: the page reports the validation error naming `combined`.
- Existing fixtures gain `combined: false`; existing tests pass otherwise unmodified.

Run: `python3 scripts/test-changed.py`.

**Constraints.** No change to `.wt/` file formats. No verdict file is written by combining.

**Depends on:** none.

**Siblings.** Task CF changes how a failed combine's output is cut and reported. It is not in this worktree; do not touch `combine.rs`, `format_record_failure` or the thread's message styles here.

**Commit message:** `fix(surface): combined clean runs leave the Review count and list`

### Task CF: a failed combine shows why it failed

**Goal.** When the check fails on a combined request, the surface and the conversation model get the failing output. Today `combine.rs` keeps the last 50 lines of the first failing step's output; a step's output is stdout followed by stderr (`Output::merged`), so for `cargo test` those 50 lines are stderr progress lines and the failing test, printed on stdout, is cut off. Later failing steps are dropped, and the message in the thread loses its line breaks.

**Files.** `crates/blyte-cli/src/combine.rs`, `crates/blyte-cli/src/requests.rs`, `crates/blyte-cli/assets/surface/index.html`, `crates/blyte-cli/tests/test_requests.rs`, `crates/blyte-cli/tests/test_surface_page.rs`, `crates/blyte-agent/src/run.rs`, `crates/blyte-agent/tests/test_gate_failures.rs`.

**What to build.**

1. `combine.rs`, the branch that builds a `CheckFailed` record from `gate_result`:
   - `gate_result.failures` is empty although the gate did not pass: return `Err("combine check failed for <request> but no failing step was recorded")`. Remove the substitute `(gate.clone(), gate_result.output)`.
   - One failure: `failing_step` is its `step`; `output_tail` is `blyte_agent::run::cut_gate_output(&f.output)`.
   - More than one: `failing_step` is every failing step's `step`, in order, joined with `"; "`; `output_tail` is `cut_gate_output(&gate_result.output)` (that text already lists each failing step with its output and any sandbox hint).
   - Leave the setup-failure branch and `tail_output` as they are.
2. `requests.rs` `format_record_failure`: return `Result<String, String>`.
   - `Conflict` with `conflicting_run` `None` or `conflicting_files` empty: `Err` naming the field and the record's path (`.wt/combined/<request>.json`).
   - `CheckFailed` with `failing_step` `None`, or `output_tail` `None` or empty: `Err` the same way.
   - `Combined`: `Err` ("not a failure record"); remove the empty string.
   - Remove `"unknown"`, `"(none listed)"` and the variant without output.
   - The caller in `maybe_combine`: on `Err(e)`, report it the way the neighbouring error arm does (`AttemptResult::Error`, `Some(e)`, `format_error_failure`).
4. `crates/blyte-agent/src/run.rs` `run_gate_step`: failing lines must survive the 400-line bound. Today the step's output is `tools::bound_output(o.merged().trim_end())`, which keeps the first 266 and last 134 lines, and only afterwards does `cut_gate_output` look for failing lines (`summary_lines`), so a failure in the dropped middle is lost. Change, for a step that exits non-zero only:
   - Collect `summary_lines` from the full `o.merged()` before bounding.
   - Keep those that are not a line of the bounded body. If there are any, the step's output is `exit <code>` (and the `gate ran out of time` line where it applies today), then a block `Failing, in lines blyte left out:` with one line each, then a blank line, then the bounded body as today.
   - If none were dropped, the output is exactly what it is today. A step that passes is unchanged.
   - `summary_lines` keeps its limit of 40 lines. Do not raise `OUTPUT_LINE_BUDGET` or change `bound_output`, `distil` or `cut_gate_output`.
   This changes what a run's own check failure shows as well as a combine's; that is intended.
3. `index.html`:
   - `.msg-line.user`: add `white-space: pre-wrap;` so a queued failure message keeps its lines in the thread.
   - `validateRequestCard`: on `conflict`, an empty `conflicting_files` is a validation error naming the field; on `check_failed`, an empty `output_tail` likewise. Remove the "No conflicting files listed." text.

No fallbacks anywhere in this task: where a field is missing, the error names it.

**Tests.**

- `combine.rs` unit tests (beside `failing_gate_records_check_failed_with_step_and_output`):
  - `a_failure_on_stdout_survives_many_stderr_lines`: gate `echo "test thing ... FAILED"; i=0; while [ $i -lt 120 ]; do echo "Running tests/t$i" >&2; i=$((i+1)); done; exit 1`. The record's `output_tail` contains `test thing ... FAILED`. (Fails on current code: verify that first.)
  - `every_failing_step_is_named`: a two-line gate where both steps fail with different output: `failing_step` contains both steps, `output_tail` contains both outputs.
  - A short failure is kept whole (the existing test still passes unmodified).
- `crates/blyte-agent/tests/test_gate_failures.rs` (through `run_gate_steps` with a real shell, as its other tests do):
  - `a_failure_in_the_dropped_middle_is_kept`: a step that prints 300 lines, then `test middle ... FAILED`, then 300 more lines, and exits 1. The failure's output contains `Failing, in lines blyte left out:` and `test middle ... FAILED`. (Fails on current code: verify that first.)
  - `a_failure_within_the_kept_lines_adds_no_block`: 10 lines, `test near ... FAILED`, exit 1: the output has no `Failing, in lines blyte left out:` and equals today's output.
  - `a_long_passing_step_adds_no_block`: 700 lines including `test x ... FAILED` as plain text, exit 0: no block.
  - `the_cut_still_names_a_failure_from_the_dropped_middle`: `cut_gate_output` of the first test's output contains `test middle ... FAILED`.
- `test_requests.rs`:
  - `a_conflict_record_without_files_is_reported_as_an_error_naming_the_field`: hand-written record, state `conflict`, `conflicting_files: []`: the queued message and the attempt record carry an error containing `conflicting_files` and the record path, and not `(none listed)`.
  - `a_check_failed_record_without_output_is_reported_as_an_error_naming_the_field`: likewise for `output_tail`.
  - `check_failed_queues_conversation_message_with_step_and_output` still passes unmodified.
- `test_surface_page.rs`:
  - `a_conflict_card_without_files_is_a_validation_error`, `a_check_failed_card_without_output_is_a_validation_error`.
  - `a_user_message_keeps_its_line_breaks`: the page source's `.msg-line.user` rule contains `white-space: pre-wrap`.
  - Update `requests_panel_renders_all_six_states_and_missing_field_errors` only where it asserted the removed placeholder text.

Run: `python3 scripts/test-changed.py`.

**Constraints.** Existing tests in `crates/blyte-agent/tests` pass unmodified. The `CombineRecord` fields and their names do not change. `cut_gate_output` is used as it is, not copied.

**Depends on:** RV (both change `requests.rs`, `index.html` and `test_surface_page.rs`). Dispatch with `--base wip/RV`.

**Siblings.** Task RV's `run_is_combined`, the `combined` field and `isUndecided` are already in this worktree; leave them as they are.

**Commit messages** (two commits): `fix(agent): failing lines survive the 400-line bound on a check step's output` for item 4 and its tests, then `fix(surface): a failed combine shows the failing output and names every failing step` for the rest.

### Task TK

**Goal.** Code run by a model (the conversation model's shell, an implementer's shell, and the
`read`/`grep` tools) must not be able to read the surface's token or blyte's API key file. Today the
token is written to `<main checkout>/.wt/surface.token`, which every sandbox profile lets the model
read, and on macOS the sandbox allows connections to `localhost`, so the model can call the surface's
routes that push a branch, open a pull request, answer an approval and record a decision. The rule this
restores: the model cannot push or open a pull request without the user's say-so.

**Depends on:** none. **Siblings:** other tasks of this plan are not in this worktree; do not add them.

**Files:** `crates/blyte-agent/src/sandbox.rs`, `crates/blyte-cli/src/serve.rs`,
`crates/blyte-cli/src/surface_notify.rs`, their tests (`crates/blyte-agent/tests/test_sandbox_read_deny.rs`,
`test_sandbox_linux.rs`, `crates/blyte-cli/tests/test_serve.rs`, `test_surface_notify.rs`,
`test_conversation_sandbox.rs`), and any doc that names `.wt/surface.token`.

**What to build.**

1. `pub fn surface_token_dir() -> Option<PathBuf>` in `sandbox.rs`: `state_blyte_dir()` joined with
   `surface`. `pub fn surface_token_path(main_checkout: &Path) -> Result<PathBuf, String>`:
   `<surface_token_dir>/<key>.token`, where `<key>` is the first 16 hex characters of a hash of the
   canonical main-checkout path that is the same in every process and every blyte version (SHA-256 if
   the workspace already depends on it, otherwise FNV-1a 64-bit written inline; never
   `std::collections::hash_map::DefaultHasher`). No state directory (no `HOME` and no `XDG_STATE_HOME`)
   is an error that names both variables. No fallbacks: never fall back to `.wt/`.
2. `serve.rs` writes the token only to `surface_token_path` (directory created with mode 0700, file
   0600, same atomic write as now) and removes it on exit as it removes the old file now. It no longer
   writes `.wt/surface.token`; on start it deletes a leftover `.wt/surface.token` if one exists.
   `.wt/surface.json` (the port) stays where it is. `surface_notify.rs` reads the token from
   `surface_token_path`; its `NoSurface`/`Failed` behaviour is unchanged.
3. Seatbelt (macOS): every profile blyte builds for model-run commands (`conversation_profile*`,
   `default_profile*`, network on or off) denies `file-read*` on `surface_token_dir()` (subpath) and on
   `$HOME/.config/blyte/env` (add it to `read_denied`). The deny must win over any `allow_read` grant:
   emit it after the allow rules, and test it.
4. Landlock (Linux): the read allow-list must not include the state directory or `~/.config/blyte`.
   If it does not today, change nothing and prove it with the test below.
5. The `read` and `grep` tools already refuse paths outside the workspace; add a test that reading the
   token path through the `read` tool is refused.

**Tests** (real sandbox; skip only where the existing sandbox tests skip, in the same way):

- macOS, conversation profile with network off: `cat <token path>` exits non-zero and its output does
  not contain the token. Same for `cat $HOME/.config/blyte/env`.
- macOS, default (implementer) profile with network on: the same two assertions.
- macOS: with the token directory passed as an `allow_read` grant, the read is still denied.
- Linux, `LandlockConfined` built by `conversation_profile_net` and by `default_profile_net`: the same
  two reads fail.
- Surface end to end (`test_serve.rs`): start the surface in a temporary repository with a temporary
  `HOME`; assert `.wt/surface.token` does not exist, that no file under `.wt/` contains the token
  string, and that `POST /api/requests/x/push`, `POST /api/requests/x/pull-request`,
  `POST /api/session/approvals/x` and `POST /api/runs/x/decision` without the token are refused with
  the existing "missing or invalid surface token" answer.
- `test_surface_notify.rs`: notifying works with the token at the new path; a leftover
  `.wt/surface.token` alone gives `NoSurface`.
- `surface_token_path` with neither `HOME` nor `XDG_STATE_HOME` set is an `Err` naming both; two calls
  for the same checkout give the same path; two checkouts give different paths.

Run: `cargo test -p blyte-agent --test test_sandbox_read_deny --test test_sandbox_linux` and
`cargo test -p blyte-cli --test test_serve --test test_surface_notify --test test_conversation_sandbox`.

**Constraints.** Existing tests pass; change a test only where it names the old token path. No
fallbacks. Do not change which network destinations the sandbox allows (a separate decision).

**Commit message:** `fix(sandbox): model-run commands cannot read the surface token or blyte's API key file`

### Task HT

**Goal.** Tests that coordinate a mock model server with a background base check through named pipes
can block for ever, because every wait is unbounded. Make every wait bounded so a broken handshake
fails the test within seconds with a message saying which step did not happen.

**Depends on:** none. **Siblings:** TK, BD, CL and MR run separately and are not in this worktree.

**Files (tests only):** `crates/blyte-cli/tests/test_dispatch_base_check.rs`,
`crates/blyte-cli/tests/test_passed_trees.rs`. No change under `src/`.

**What to build.**

1. A helper in each file (or one shared test-support module if these tests already share one) that
   replaces the blocking `std::fs::read_to_string(fifo)` and `std::fs::write(fifo, ..)` calls made
   inside the mock server's handler: wait at most 15 s for the peer (regular marker files polled every
   20 ms, or a non-blocking FIFO open with a deadline) and panic with a message naming the step
   ("base check never reached its gate step", "base check never took the release") on timeout. The
   gate shell command changes to match the helper.
2. The mock servers (`model_server_with`, and the `TcpListener` loop in `test_passed_trees.rs`) set a
   10 s read timeout on each accepted stream.
3. `dispatch_command` in `test_dispatch_base_check.rs` sets `BLYTE_MODEL_TIMEOUT_SECS=10` and
   `BLYTE_RETRY_DELAY_MS=10` and the dispatch calls in these tests pass `--max-seconds 60`, if those
   settings exist in `src/` under those names (check; if a name differs use the real one and say so in
   the final message).
4. In `test_dispatch_failing_base_check_when_change_passes_verifies_with_notice_and_skips_base_record`
   the handler's `else` branch that releases the base check becomes `else if n == 2`, with a plain
   `else` returning the done body, so a third request cannot wait on the pipe again.
5. Applies to: `test_dispatch_first_model_call_happens_before_base_check_ends`,
   `test_dispatch_failing_parallel_base_gate_blocks_and_cleans_up`,
   `test_dispatch_pending_base_check_stopped_without_orphan_process_and_removes_base_wt`,
   `test_dispatch_failing_base_check_when_change_passes_verifies_with_notice_and_skips_base_record`,
   and `dispatch_runs_base_check_when_base_tree_not_recorded`.

**Tests.** Each of the five still asserts what it asserted before (do not weaken an assertion). Run
each file 5 times in a row: `for i in 1 2 3 4 5; do cargo test -p blyte-cli --test test_dispatch_base_check --test test_passed_trees || exit 1; done`.

**Commit message:** `test: bound every wait in the base-check handshake tests`

### Task BD

**Goal.** When the conversation model starts `blyte dispatch` in the background (the `blyte` tool in
`crates/blyte-agent/src/tools.rs`, the `if background` branch), a dispatch that fails straight away
(bad arguments, plan task not found, no API key, id already used) is reported as "started": its output
goes to `/dev/null` and its exit status is dropped. Report early failures, and keep the output of late
ones.

**Depends on:** none. **Siblings:** TK, HT, CL and MR run separately and are not in this worktree.

**Files:** `crates/blyte-agent/src/tools.rs` and its tests.

**What to build.**

1. The background child's stdout and stderr go to one file, `<root>/.wt/<id>.dispatch.out` (created
   with truncate; if `.wt` cannot be created or the file cannot be opened, return an error naming the
   path: no fallback to `/dev/null`).
2. After spawning, wait up to 2 s for the child (poll `try_wait` every 50 ms). If it exits within that
   time with a non-zero status, return `ToolOutcome::err` with `dispatch <id> failed to start (exit N)`
   followed by the file's content bounded by the existing output bound. If it exits 0 within that time,
   return ok with `dispatch <id> finished` and the bounded content. Otherwise return the existing
   "started <id> in the background; use wait to learn when it ends" message, extended with the path of
   the output file, and keep the reaper thread.
3. No fallbacks: an error from `try_wait` is returned as an error.

**Tests** (in the tools tests, using a stub executable the way existing tests of this tool stub `blyte`;
if none do, add a test-only way to set the executable path):

- stub exits 3 after printing `no API key` to stderr: outcome is an error containing `exit 3` and
  `no API key`; `.wt/<id>.dispatch.out` contains `no API key`.
- stub sleeps 5 s then exits 0: outcome is ok, contains `started` and the output file path, and returns
  in under 4 s.
- stub prints `ok` and exits 0 at once: outcome is ok and contains `finished`.
- `.wt` is a regular file (so the output file cannot be created): outcome is an error naming the path.

Run: `cargo test -p blyte-agent`.

**Commit message:** `fix(agent): a background dispatch that fails to start is reported, and its output is kept`

### Task CL

**Goal.** Recording a verdict removed a run's worktree and branch although its commit was on no other
branch. Find the cause in `crates/blyte-cli/src/clean.rs` (the merged test: `check_run_merged`,
`decide_run_merged`, `resolve_base_targets`, and the verdict path that prints
`removed .wt/<id>; branch wip/<id> deleted`) and fix it: a run is removed as merged only when its own
commits are in the default branch or in the branch it was dispatched from.

**Depends on:** none. **Siblings:** TK, HT, BD and MR run separately and are not in this worktree.

**Files:** `crates/blyte-cli/src/clean.rs` and its tests. Nothing else unless the cause is elsewhere;
then say so in the final message.

**The case that went wrong (reproduce it as a test first; it must fail before the fix).**

1. Default branch `main`. Branch `F` is created from `main`.
2. Run `A` is dispatched with base `F` (its start file records `"base": "F"`) and commits `a` on `wip/A`.
3. `git cherry-pick -x a` onto `F`. Recording `A`'s verdict removes `.wt/A` and `wip/A` (correct:
   merged into `F`).
4. Run `B` is dispatched with base `wip/A` (start file `"base": "wip/A"`) and commits `b` on `wip/B`
   with parent `a`. `b` is cherry-picked nowhere.
5. Recording `B`'s verdict `accepted` must print `worktree .wt/B kept until wip/B is merged` and leave
   `wip/B`. It removed both.

If this exact case does not fail on the current code, vary it (for example `A`'s verdict `fixed`, `A`'s
worktree removed but start file present, `B`'s start file base as a commit id) until the removal
reproduces, and state in the final message which variation did. If none does after an honest attempt,
stop and say so; do not change the merged test without a failing test.

**Also test:** after `git cherry-pick -x b` onto `F`, recording `B`'s verdict removes it; and the
existing clean tests pass unchanged.

Run: `cargo test -p blyte-cli clean`.

**Commit message:** `fix(clean): a run whose base branch is gone is not removed until its own commits are merged`

### Task MR

**Goal.** A request whose work was combined and then merged and cleaned up (its runs' `wip/` branches
removed by blyte's cleanup) shows in the surface's Requests view as `error` for ever ("branch ... does
not resolve to a commit"), and an error is recorded on every poll. Give such a request a settled state.

**Depends on:** none. **Siblings:** TK changes the surface token code in `serve.rs`; HT, BD and CL are
elsewhere. None are in this worktree. Keep changes to `serve.rs` to what this task needs.

**Files:** `crates/blyte-cli/src/requests.rs`, `crates/blyte-cli/assets/surface/index.html`, tests in
`crates/blyte-cli/tests/test_requests.rs`, `test_serve.rs`, `test_surface_page.rs`.

**What to build.**

1. In the request status computation (the `Readiness::Ready` arm that calls
   `resolve_current_run_tips`, and any other place a request's status needs its runs' tips): before
   resolving tips, if the request has a combine record in state `combined` and every run of the request
   is combined by the existing per-run rule (`run_is_combined` / `combined_run_ids`: listed in the
   record, and its branch is gone or still at the recorded tip) and at least one of its branches is
   gone, the state is `done`: no error, no combine attempt, no error recorded for the session. The
   status carries the record's metadata as the `combined` state does.
2. Anything else stays loud: a missing branch for a run that the record does not list, or a request
   with no `combined` record, is still an `error` naming the run and branch. No fallbacks.
3. A `done` request offers no push or pull-request action: the push and pull-request routes refuse it
   with a message saying the request's branches were already merged and removed.
4. Page: `done` is a known state, labelled `Done`, shown without an error style, with no Combine, Push
   or Pull request buttons, and sorted after requests in every other state. An unknown state is still
   rejected by the page's validation as now.

**Tests.**

- `test_requests.rs`: combined record, all branches deleted -> `done`, `error` is `None`. Combined
  record, one of two branches deleted and the other at its recorded tip -> `done`. Combined record, a
  branch deleted for a run the record does not list -> `error` naming that run. No record, branch
  deleted -> `error` as now. Combined record, no branch deleted -> unchanged from now.
- `test_serve.rs`: polling `/api/requests` twice for a `done` request records no session error; push
  and pull-request on it are refused with the message above.
- `test_surface_page.rs`: a `done` request renders the label `Done`, no action buttons, after a
  `waiting` request; a request with state `bogus` is still rejected.

Run: `cargo test -p blyte-cli --test test_requests --test test_serve --test test_surface_page`.

**Commit message:** `fix(surface): a request whose branches were merged and removed shows as done, not as an error`
