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
