# Surface follow-ups: honest entry history, combine on finish, clearer check failures

Date: 2026-11-24. Follows #412. One pull request for all tasks.

## Why

1. A failed write to `.wt/sessions/entries.jsonl` is reported only on stderr and leaves a gap that a
   restart shows as a complete history.
2. The surface finds finished runs by polling every 2 s. A run can tell the surface it finished.
3. When a check fails for an environment reason, `blyte review` and `.wt/<id>.json` hide why:
   - a run blocked during setup prints an empty `== report`;
   - a blocked reason held only in `error` is never printed;
   - `gate_failures` are never printed by `blyte review`;
   - `.wt/<id>.json` keeps the first 1,000 characters of each failure, which drops the failures
     section at the end;
   - nothing says that "Operation not permitted" usually means the sandbox denied a path outside
     the worktree (seen with a `.env` in the main checkout).

## Facts checked (research run RS3, 2026-11-24, against 86e0cb74)

- Entries are numbered in one place, `SessionState::push_entry` (`session.rs`), `last_seq += 1`, so
  in memory they are 1, 2, 3, ... A failed `EntryLog::append` still advances the number, so the file
  gets a gap. Loading checks no order. `EntryBody::Error { message }` renders on the page as
  `.entry-error`.
- `.wt/surface.json` holds the surface's `port` and `session_id`; `.wt/surface.token` holds the
  token. `ureq` is already a dependency of `blyte-cli`. `reconcile_runs` and
  `spawn_reconcile_timer` are in `serve.rs`.
- `blyte review` reads `.wt/<id>.report.json` only (`queue_actions::compute_review`,
  `dispatch::run_review`). A run blocked at setup has no `.report.json`.
- `parse_pass_report` (`dispatch.rs`) keeps `s.chars().take(1000)` of each `gate_failures` entry.
  `cut_gate_output` (`run.rs`) already builds a 4,000-character entry: header, summary lines, head
  and tail.
- Gate output for every caller comes from `run_gate_steps_cancellable` / `run_gate_step` (`run.rs`).

## Rules for every task

- No fallbacks: when something is missing or does not match, fail with an error that names it.
  Never substitute a default, a guess, a silent skip or an empty result. The one stated exception
  is in Task NT1 (no surface running is a normal state, not an error).
- Existing tests pass unmodified unless the task says otherwise.
- Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings`, then the tests the
  task names.
- No new dependencies.

### Task EL

**Goal.** A gap in the saved session entries is never shown as a complete history, and a failed
write is visible on the page.

**Depends on:** none.

**Siblings.** CK changes `run.rs`, `dispatch.rs`, `queue_actions.rs`; NT1 changes `serve.rs` and
adds `surface_notify.rs`. They are not in this worktree; do not touch those files.

**Files.** `crates/blyte-cli/src/session.rs`, `crates/blyte-cli/tests/test_session.rs`.

**What to build.**

1. Failed write, at run time. When `EntryLog::append` fails (serialise or write), the session
   carries on, as now, and the stderr line stays. In addition, the first time it fails in a
   process, push one `EntryBody::Error` entry with the message
   `blyte: could not save this session's entries to <path>: <error>; entries from here on may be missing after a restart`.
   Push it once per process, not once per failure. The notice entry is itself offered to the log
   in the normal way; if that write fails too it must not push another notice (no recursion, no
   loop). Make `append` return a `Result` so `push_entry` can decide.
2. Order check at load. After parsing, the `seq` values must be strictly increasing and at least 1.
   A `seq` that is not greater than the one before it is corruption: return an error
   `invalid entry in <path> at line <n>: seq <x> does not follow seq <y>` and stop the start, as a
   bad line in the middle does today.
3. Gap notice at load. Where `seq` jumps by more than 1 (from `a` to `b`, `b > a + 1`), or where
   the first entry's `seq` is greater than 1 (treat `a` as 0), insert in memory, at that position,
   one `Entry { seq: a + 1, ts: <ts of the entry with seq b>, body: EntryBody::Error { message } }`
   with the message `blyte: <b - a - 1> entries were not saved here` (write `1 entry was not saved
   here` for one). Do not write gap notices to the file: they are derived on every load. The
   numbering of new entries continues from the highest loaded `seq`, as now.
   `entries_after(after)` must return the notices in order like any other entry.

**Tests** (`test_session.rs`; follow the existing `start_with_log_io` tests).

- `a_failed_write_shows_one_notice_on_the_page`: start with a log, then make writes fail (for
  example replace the log path's parent with a setup where the open file can no longer be written;
  if no portable way exists, add a `#[doc(hidden)] pub` test seam on the log that makes the next
  writes fail). Push three entries. Exactly one `Error` entry with `could not save` is in the
  entries, and all three pushed entries are present.
- `a_gap_in_the_saved_entries_is_shown_at_load`: file with seq 1, 2, 5. Loaded entries are seq
  1, 2, 3 (Error, `2 entries were not saved here`), 5, then the restart notice at 6. The file's
  first three lines are unchanged.
- `a_missing_first_entry_is_shown_at_load`: file with seq 2, 3. First loaded entry is seq 1,
  `1 entry was not saved here`.
- `entries_out_of_order_stop_the_start`: file with seq 1, 3, 2. Start fails; the error names the
  file and `line 3`.
- `a_repeated_seq_stops_the_start`: file with seq 1, 1. Start fails naming `line 2`.

Run: `cargo test -p blyte --test test_session`.

**Commit message.** `fix(surface): show a failed entry save on the page and a gap in saved entries at load`

### Task CK

**Goal.** When a check fails, `blyte review` and `.wt/<id>.json` show why, including the end of the
output, and say when the sandbox is the likely cause.

**Depends on:** none.

**Siblings.** EL changes `session.rs`; NT1 changes `serve.rs` and adds `surface_notify.rs`. Not in
this worktree; do not touch those files. A later task (NT2) will add a call in `dispatch.rs` where
the summary file is written; leave that code's structure as it is.

**Files.** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/dispatch.rs`,
`crates/blyte-cli/src/queue_actions.rs`, and tests: `crates/blyte-agent/tests/test_gate_failures.rs`
(or the existing test file that covers `cut_gate_output`), new `crates/blyte-cli/tests/test_review_reasons.rs` (model its setup on
`crates/blyte-cli/tests/test_review_verdict.rs`), and `crates/blyte-cli/tests/test_report_gate_failures.rs`
or a `#[cfg(test)]` module for the summary cut.

**What to build.**

1. Sandbox hint (`run.rs`). Add
   `pub fn sandbox_denial_hint(output: &str) -> Option<&'static str>` returning, when `output`
   contains `operation not permitted` in any letter case, exactly:
   `hint: "Operation not permitted" usually means blyte's sandbox denied a path outside the worktree (for example a .env in the main checkout). Make the check independent of files outside the worktree.`
   and `None` otherwise. In `run_gate_steps_cancellable` (the one place every caller's result
   comes from), when the gate failed and the hint applies, append `\n` + the hint as the last line
   of `GateRunResult.output` and of the last failing step's `GateStepFailure.output`, after
   bounding, so it is never cut. Append it once: output that already ends with the hint is left
   alone. A passing gate gets no hint.
   `cut_gate_output` and `base_check_tail` must keep the hint line (both keep the tail today;
   prove it with tests).
2. Keep the end of each failure in `.wt/<id>.json` (`dispatch.rs`, `parse_pass_report`). Replace
   the 1,000-character head cut. An entry of 1,000 characters or fewer is kept whole. A longer one
   becomes: its first line (cut to 200 characters), a line `[... <n> characters left out ...]`, then
   the last characters of the entry up to a total of 1,000, starting at a line boundary where one
   exists inside that window. Count characters, not bytes; never split a character.
3. `blyte review` (`queue_actions.rs` `compute_review`, `dispatch.rs` `run_review`).
   - When `.wt/<id>.report.json` does not exist, read `.wt/<id>.json` and print under `== report`
     its `outcome`, and `error` when present. If neither file exists, keep today's behaviour for
     an unknown run (do not invent a new one).
   - When the report has an `error` that is non-empty and is not contained in `final_message`,
     print it as an `error:` line under `== report`, after `outcome`.
   - When the report's `gate_failures` is non-empty, print a section `== check failures` holding
     the last entry only, whole (it is at most about 4,000 characters), and a first line
     `(<n> failed attempts; the last one)` when there was more than one.
   - JSON output modes of review, if any, get the same fields; do not change existing field names.

**Tests.**

- `run.rs` side: `the_hint_is_added_once_to_a_failed_check_that_was_denied` (a gate step
  `echo 'cat: /x/.env: Operation not permitted'; exit 1` gives output whose last line is the hint;
  running the result through the append again does not duplicate it);
  `a_passing_check_gets_no_hint` (same echo, `exit 0`);
  `a_failed_check_without_the_phrase_gets_no_hint`;
  `the_hint_survives_the_stored_cut` (5,000 lines of output then the phrase: `cut_gate_output`
  result ends with the hint) and `the_hint_survives_the_base_check_tail`.
- `dispatch.rs` side: `a_long_check_failure_keeps_its_end_in_the_summary` (an entry of 4,000
  characters whose last line is `FAILED tests/x.py::test_y`: the summary entry is at most 1,000
  characters plus the marker line, starts with the entry's first line, contains the marker with the
  right count, ends with `FAILED tests/x.py::test_y`); `a_short_check_failure_is_kept_whole`;
  `a_cut_never_splits_a_character` (entry of 3,000 `é`).
- review side: `review_shows_why_a_run_was_blocked_at_setup` (only `.wt/<id>.json` with
  `outcome: blocked` and an `error`: `== report` shows both);
  `review_shows_an_error_the_final_message_does_not_contain`;
  `review_does_not_repeat_an_error_already_in_the_final_message`;
  `review_shows_the_last_check_failure` (two `gate_failures` entries: the section has the count
  line and the second entry, not the first).

Run: `cargo test -p blyte-agent && cargo test -p blyte --test test_review_reasons --test test_report_gate_failures` plus any other test file
you added to.

**Commit message.** `fix(review): show why a check failed: blocked reasons, the last failure, its end, and a sandbox hint`

### Task NT1

**Goal.** The surface combines finished runs when it is told a run finished, and polls only as a
safety net.

**Depends on:** none.

**Siblings.** EL changes `session.rs`; CK changes `run.rs`, `dispatch.rs`, `queue_actions.rs`. Not in
this worktree; do not touch those files. NT2 will call the helper below from `dispatch.rs` later;
do not add that call here.

**Files.** `crates/blyte-cli/src/serve.rs`, new `crates/blyte-cli/src/surface_notify.rs` (register
the module where the crate's other modules are registered), `crates/blyte-cli/src/direct_edit.rs`
(one call), `crates/blyte-cli/tests/test_serve.rs`, new
`crates/blyte-cli/tests/test_surface_notify.rs`, and the surface section of
`docs/reference.md` if it lists the routes.

**What to build.**

1. Route `POST /api/runs/changed` in `serve.rs`. It needs the token like every other `/api/*`
   route (403 without it; add it to whatever table the token test iterates so it is covered).
   It wakes the reconcile thread and answers `202` with body `{}` straight away; it does not run
   `reconcile_runs` on the request thread. Several posts while a reconcile is running lead to one
   more reconcile after it, not one per post.
2. The reconcile timer's interval becomes 30 s (it is 2 s). Keep the wake on session idle. Name the
   constant so its role is clear (a safety net for a run that ended without telling the surface).
3. `surface_notify.rs`:
   `pub fn notify_runs_changed(main_checkout: &Path, id: &str) -> NotifyOutcome` with
   `pub enum NotifyOutcome { NoSurface, Told, Failed(String) }`.
   - If `.wt/surface.json` or `.wt/surface.token` does not exist: `NoSurface`. This is the normal
     state when no surface is running, and the only case that is silent.
   - If either file exists but cannot be read or parsed: `Failed` naming the file.
   - Otherwise POST to `http://127.0.0.1:<port>/api/runs/changed` with the token sent the way the
     page sends it, connect and total timeout 2 s, using `ureq` or the raw `TcpStream` style already
     in `serve.rs`. `202` gives `Told`. Anything else (refused connection, timeout, another
     status) gives `Failed("could not tell the surface at 127.0.0.1:<port> that <id> finished: <reason>; it checks again within 30 s")`.
   - Never print the token, and never put it in an error message.
   `pub fn report(outcome: &NotifyOutcome, err: &mut dyn Write)` writes `blyte: <message>` for
   `Failed` and nothing otherwise. The helper never changes its caller's exit code.
4. `direct_edit.rs`: after `edit-finish` writes `.wt/<id>.json` for a finished edit, call
   `notify_runs_changed` and `report` to stderr.

**Tests.**

- `test_serve.rs`: `runs_changed_needs_the_token` (403 without, 202 with);
  `a_runs_changed_post_combines_without_waiting_for_the_timer` (start a surface with the 30 s
  timer, write a finished run's files the way the existing reconcile tests do, POST, and the run is
  reconciled within 5 s); `the_safety_net_interval_is_30_seconds` (assert the constant).
- `test_surface_notify.rs`: `no_surface_files_is_silent` (`NoSurface`, `report` writes nothing);
  `a_running_surface_is_told` (against a real surface from the `test_serve.rs` helpers, or a
  minimal local listener that records the request: the path is `/api/runs/changed`, the token is
  sent, outcome `Told`); `a_dead_surface_is_reported_not_fatal` (`surface.json` pointing at a closed
  port: `Failed`, the message names the port and the id and does not contain the token);
  `an_unreadable_surface_file_is_reported` (`surface.json` containing `not json`: `Failed` naming
  the file); `a_wrong_token_is_reported` (listener answers 403: `Failed`).

Run: `cargo test -p blyte --test test_serve --test test_surface_notify --test test_direct_edit`.

**Commit message.** `feat(surface): combine when a run reports it finished; poll every 30 s as a safety net`

### Task NT2

**Goal.** `blyte dispatch` tells a running surface when its run has finished.

**Depends on:** CK and NT1 (both change files this task touches or calls). Base: the branch that
has both.

**Files.** `crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/tests/test_dispatch_notify.rs` (new).

**What to build.** Every place `blyte dispatch` writes the final `.wt/<id>.json` (setup blocked,
the fix-round error paths, normal completion) calls
`surface_notify::notify_runs_changed(<main checkout>, id)` after the file is in place, then
`surface_notify::report(&outcome, &mut std::io::stderr())`. Route the writes through one function
if they are not already, so a new outcome cannot forget the call. The call never changes the
dispatch's exit code or its stdout (stdout stays the one JSON object). Research runs
(`--research`) notify too. Paths that exit before any summary file is written do not notify.

**Tests** (`test_dispatch_notify.rs`, using the fake model server helpers the other dispatch tests
use; copy the minimal helpers, do not move them).

- `dispatch_tells_the_surface_when_it_finishes`: a temp repository with `.wt/surface.json`
  (port of a local listener the test owns) and `.wt/surface.token`. Run a dispatch to completion.
  The listener received exactly one `POST /api/runs/changed` carrying the token, and at the moment
  it arrived `.wt/<id>.json` existed (check inside the listener).
- `dispatch_without_a_surface_prints_nothing_about_it`: no surface files; stderr does not contain
  `surface`; exit code and stdout unchanged.
- `a_dead_surface_does_not_change_the_result`: `surface.json` points at a closed port; the outcome
  and exit code equal those of the run without a surface, stderr has one line starting
  `blyte: could not tell the surface`.
- `a_blocked_run_tells_the_surface_too`: a gate that fails at the base check; one POST received.

Run: `cargo test -p blyte --test test_dispatch_notify --test test_dispatch_base_check`.

**Commit message.** `feat(dispatch): tell a running surface when the run has finished`
