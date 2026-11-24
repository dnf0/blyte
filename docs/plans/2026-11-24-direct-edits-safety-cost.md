# Direct edits for small changes, two safety fixes, cost by session, and tidy-up

Date: 2026-11-24. One pull request. Facts below were checked against `main` at 2bb2fb6 by a
read-only research run; each task names the lines it starts from.

## Decisions

1. **Direct edits are for small changes only.** The orchestrating model edits directly when the
   change is documentation only, or is confined to one file. Anything larger goes to a run. The
   prompt says so and `blyte edit-finish` enforces it. Evidence: `docs/evidence.md`, "A working
   session: one model does everything, against the default process" (half the cost, fewer than
   half the tests, a larger context).
2. **A direct edit gets the same scrutiny as a run.** `edit-finish` always asks for the fresh
   model review, and sends its findings back to the editor once (one fix round) before the result
   goes to the user's queue.
3. **The conversation shell can write in an open edit worktree**, and nowhere else new.
4. **Every `/api/*` route of the surface needs the token**, except a liveness route that gives
   nothing away.
5. **A model cannot write the records that say a check passed or that the user accepted.**
6. **`blyte cost` reports by session**, and the fresh model review's cost is in the ledger.
7. **Tidy-up:** combining does not wait for a page request; the old name leaves the terminal;
   the surface keeps its entries across a restart; tests cannot reach the real usage ledger.

## Rules for every task

- Existing tests pass unmodified unless the task names the test and says how it changes.
- **No fallbacks:** when something is missing, unknown or does not match, fail with an error that
  names it. No default standing in for "unknown", no silent skip, no empty result.
- Nothing here is a long job, so "resumable" applies only where a task says so.
- Change only the files the task names. If the task cannot be done within them, stop and say why.
- No old product names, no company or private repository names anywhere.
- The check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && python3 scripts/test-parallel.py -- --workspace --all-targets`.

## Order

`DE`, `SH1`, `SV1`, `CO`, `MISC` have no dependencies and run together. `SH2` follows `SH1`;
`SV2` follows `SV1`; `SV3` follows `SV2`.

---

### Task DE: direct edits only for small changes, with a model review and one fix round

**Depends on:** none. **Siblings (not in this worktree; do not do their work):** SH1, SH2 (shell
and sandbox), SV1–SV3 (`serve.rs`, `session.rs`, the page), CO (`model_review.rs`, `cost.rs`,
`cli.rs`), MISC (`repl.rs`, `.cargo/config.toml`, `usage_log.rs`).

**Why.** Direct edits are on for every orchestrated session, but `process.md` never mentions them
and nothing limits their size; and `edit-finish` runs the gate but no model review and no fix
round, which a dispatched run gets.

**Files:** `crates/blyte-cli/src/direct_edit.rs`, `crates/blyte-cli/src/queue_actions.rs`,
`crates/blyte-cli/assets/orchestrator/process.md`, `crates/blyte-cli/assets/orchestrator/reference.md`,
`crates/blyte-cli/tests/test_direct_edit.rs`, `crates/blyte-cli/tests/test_orchestrator_reference.rs`.
Do not change `model_review.rs`, `fix_round.rs` or `dispatch.rs`; call what they export.

**What to build.**

1. *Scope rule in code.* In `run_edit_finish_in`, right after `collect_changed_files` (about
   `direct_edit.rs:507-513`) and before the gate, add
   `pub fn direct_edit_scope(changed: &[String]) -> Result<(), String>`:
   - `Ok` when `changed` has at most one path;
   - `Ok` when every path is documentation: extension `md`, `mdx`, `rst`, `txt` or `adoc`
     (case-insensitive), or the path starts with `docs/`, or the file name starts with `README`,
     `CHANGELOG` or `LICENSE`;
   - otherwise `Err` with exactly this shape: `direct edits are for documentation or a change
     confined to one file; this edit changes N files (<comma-separated paths>): hand it to a run
     with blyte dispatch`.
   On `Err`, `edit-finish` prints `{ "id", "outcome": "out_of_scope", "changed_files", "error" }`,
   leaves the edit open (`.wt/<id>.edit.json` stays, no `.wt/<id>.json` is written) and exits 2.
   The gate must not have run.
2. *Model review, always.* Give `evaluate_verified_run_with` in `queue_actions.rs` a parameter that
   forces the fresh model review whatever the usual policy says; `evaluate_verified_run` keeps its
   present behaviour for dispatched runs. `edit-finish` uses the forced form after the gate passes.
3. *One fix round, by the editor.* After the review, if it has findings that would make a
   dispatched run wait in the user's queue or start its fix round (confirmed fallbacks or
   concerns; use the existing `fix_round::should_run` and the review's own result, do not invent a
   new test), and `.wt/<id>.fixround.json` does not exist: write that record with the existing
   `fix_round::write_record`, print `{ "id", "outcome": "review_findings", "findings": [...] }`
   with each finding's file, line and text as the review gave them, leave the edit open and exit 1.
   The next `edit-finish` for the same id (the record now exists) runs scope, gate and review
   again and then finishes whatever the review says: it writes the summary, and any findings
   still open stay as flags on the run so it waits in the user's queue exactly as a dispatched run
   with open findings does. Never more than one such round. If the review itself fails (provider
   error, unparseable answer), record that as dispatched runs do and finish; do not treat a failed
   review as a clean one.
4. *Prompt.* In `process.md`, under the part that says what the orchestrator does itself, add a
   short `### Direct edits` section: edit directly only when the change is documentation, or is
   confined to one file; hand anything larger to one run; `edit-finish` refuses a larger edit;
   after `review_findings`, fix what the review found and call `edit-finish` again, once. Keep the
   existing sentence that work goes to one run, and make the two consistent. In `reference.md`
   (the `blyte edit-start` / `blyte edit-finish` paragraphs, about lines 35-50) document the scope
   limit, the `out_of_scope` and `review_findings` outcomes and their exit codes, and replace
   "the shell cannot write there" with "the shell and the write tools can write there" (task SH2
   makes the shell part true; say nothing else about the shell).

**Tests** (in `test_direct_edit.rs` unless said; model the review with the fake provider the
existing fresh-review tests use):
- `scope_allows_one_file_and_documentation_only`: unit cases for `direct_edit_scope`: `[]`,
  `["src/a.rs"]`, `["docs/x.md","README.md","notes.TXT"]`, `["docs/a.png","docs/b.md"]` are `Ok`;
  `["src/a.rs","src/b.rs"]` and `["src/a.rs","README.md"]` are `Err` with the message shape above,
  naming both paths and the count.
- `a_two_file_code_edit_is_refused_before_the_gate`: a gate that writes a marker file; finish
  exits 2 with `out_of_scope`; the marker does not exist; `.wt/<id>.edit.json` exists and
  `.wt/<id>.json` does not.
- `a_direct_edit_is_always_reviewed`: a clean one-file edit whose task names no files (so the
  usual policy would skip the review): `.wt/<id>.model-review.json` exists afterwards.
- `review_findings_come_back_once_then_the_edit_finishes`: the review reports a confirmed
  fallback: first finish exits 1 with `review_findings` and the finding's file and line, and no
  `.wt/<id>.json`; second finish with the review still reporting it exits 0, writes
  `.wt/<id>.json`, and `.wt/<id>.checks.json` carries the flag.
- `a_failed_review_is_recorded_not_passed`: the provider errors: finish writes the summary and the
  checks record the failed review.
- In `test_orchestrator_reference.rs`: `default_process_says_when_to_edit_directly`: `process.md`
  contains both "confined to one file" and "documentation", and still tells the model to hand
  other work to one run. The existing `default_process_hands_a_request_to_one_run_and_ends_the_turn`
  stays unmodified and passing.

Run: `cargo test -p blyte --test test_direct_edit --test test_orchestrator_reference --test test_fresh_model_review --test test_fix_round`.

**Commit message:** `feat(edit): direct edits for small changes only, with a model review and one fix round`

---

### Task SH1: a model's shell cannot write the records of a check or of the user's decision

**Depends on:** none. **Siblings (not here):** DE, SV1–SV3, CO, MISC; SH2 follows this task.

**Why.** From its shell, or through the `blyte` tool, the orchestrating model can run
`blyte review <id> --verdict accepted`, `blyte combine` and `blyte review --model-review`, which
write the user's verdict, the combine and passed-tree records, and the review record. A sandboxed
shell can also write the usage ledger, because the sandbox allows all of `$HOME/.local`
(`sandbox.rs:305` and `:414`).

**Files:** `crates/blyte-agent/src/shell.rs`, `crates/blyte-agent/src/sandbox.rs`,
`crates/blyte-agent/src/tools.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-agent/tests/test_blyte_tool.rs`, new `crates/blyte-cli/tests/test_model_shell_refusals.rs`,
`crates/blyte-cli/tests/test_orchestrator_sandbox.rs` (additions only).

**What to build.**

1. *Marker.* Every shell a model's commands run in gets the environment variable
   `BLYTE_MODEL_SHELL=1`: the `user` session of `PersistentShell`, `LocalShell::exec_user`, and
   `exec_host` when it runs a model's `bash` command. It is never set on the harness session
   (`Shell::exec`, the gate), on `dispatch::run_implementer_pass`, on `clean::spawn_background_clean`,
   or on the process the `blyte` tool spawns (`tools.rs:1345`); if the parent has it set, those
   four remove it from the child.
2. *Refusals in `main.rs`*, right after the arguments are parsed: when `BLYTE_MODEL_SHELL` is set
   to `1`, exit 2 with `error: blyte <command> writes a record only the user or the harness may
   write, and cannot be run from a model's shell` for: `review` with `--verdict accepted` or
   `--verdict fixed`; `review --model-review`; `combine`. `review --verdict redone` stays allowed
   (the default process uses it). Everything else is unchanged.
3. *The `blyte` tool* (`tools.rs`, the arm about line 1310-1345): refuse the same three forms
   before spawning, with the same message. Other commands unchanged.
4. *The ledger.* A sandboxed shell must not be able to write `<state dir>/blyte` (that is
   `$XDG_STATE_HOME/blyte` when set, else `$HOME/.local/state/blyte`). On macOS, add a
   `(deny file-write* (subpath "<that dir>"))` after the allows in every profile that allows
   `$HOME/.local`. On Linux (Landlock cannot deny below an allow), replace the `$HOME/.local`
   grant with one grant per existing entry of `$HOME/.local` except `state`, plus one per existing
   entry of `$HOME/.local/state` except `blyte`. Reads are unchanged.
5. Before changing anything, read `crates/blyte-cli/assets/orchestrator/process.md` and
   `reference.md`: if either tells the orchestrator to run a form this task refuses, stop and
   report it; do not edit those files.

**Tests.**
- `test_model_shell_refusals.rs`: `verdict_accepted_and_fixed_are_refused_in_a_model_shell`,
  `combine_and_model_review_are_refused_in_a_model_shell` (each: run the binary with
  `BLYTE_MODEL_SHELL=1` in a temp repository; exit 2; stderr has the message; no
  `.wt/<id>.verdict.json` / `.wt/combined` written); `verdict_redone_is_allowed_in_a_model_shell`;
  `the_same_commands_work_without_the_marker` (verdict accepted is recorded).
- `test_blyte_tool.rs`: `the_tool_refuses_verdict_accepted_fixed_combine_and_model_review` (the
  fake executable is not started) and `the_tool_child_does_not_carry_the_marker` (parent env has
  `BLYTE_MODEL_SHELL=1`; the fake executable records its environment; the marker is absent).
- A unit or integration test `a_models_command_sees_the_marker_and_a_harness_command_does_not`
  (`echo "$BLYTE_MODEL_SHELL"` through the user session prints `1`; through the harness session
  prints an empty line).
- `test_orchestrator_sandbox.rs`: `sandboxed_shell_cannot_write_the_usage_ledger_directory`
  (with `HOME` pointed at a temp directory holding `.local/state/blyte` and `.local/bin`: a write
  into `.local/state/blyte/x` fails, a write into `.local/bin/x` succeeds). Skip only where the
  existing sandbox tests in that file skip.

Run: `cargo test -p blyte-agent && cargo test -p blyte --test test_model_shell_refusals --test test_orchestrator_sandbox --test test_review_verdict`.

**Commit message:** `fix(sandbox): a model's shell cannot record a verdict, a combine or a review, or write the usage ledger`

---

### Task SH2: the conversation shell can write in an open edit worktree

**Depends on:** SH1 (dispatch with `--base wip/SH1`). **Siblings (not here):** DE (which already
changes `reference.md` to say the shell can write there; do not touch `reference.md`), SV1–SV3,
CO, MISC.

**Why.** After `blyte edit-start`, the write tools can write in `.wt/<id>` but the orchestrator's
sandboxed shell cannot, so the model cannot run a formatter or a code generator on its own edit.
An OS sandbox profile is fixed when the shell starts, so the shell has to be restarted with a new
profile when the set of open edits changes.

**Files:** `crates/blyte-agent/src/sandbox.rs`, `crates/blyte-agent/src/shell.rs`,
`crates/blyte-cli/src/task_runner.rs`, `crates/blyte-cli/src/orchestrator_text.rs`,
`crates/blyte-cli/tests/test_orchestrator_sandbox.rs`.

**What to build.**
1. `sandbox.rs`: `pub fn open_edit_worktrees(main_checkout: &Path) -> Result<Vec<PathBuf>, String>`:
   the directories `<main>/.wt/<id>` for which `<main>/.wt/<id>.edit.json` exists,
   `<main>/.wt/<id>.json` does not, and the directory exists (the same rule as
   `is_in_open_edit_worktree`; share the code). Sorted. An unreadable `.wt` is an `Err` naming it;
   a missing `.wt` is an empty list.
2. The orchestrator's profile takes those directories as extra writable paths. On macOS, after
   each `(allow file-write* (subpath "<wt>"))` add `(deny file-write* (literal "<wt>/.git"))`.
   `<main>/.git` stays unwritable, so `git commit` in the worktree still fails.
3. `shell.rs`: `PersistentShell` takes an optional profile builder
   (`Arc<dyn Fn() -> Result<Profile, String> + Send + Sync>`). Before each command in the `user`
   session it builds the profile and, only if it differs from the profile the session was started
   with, closes that session and starts a new one with the new profile. The shell's working
   directory is carried over to the new session; exported variables are not, and the command's
   output is prefixed with one line `blyte: the shell restarted because the open edits changed;
   exported variables were reset`. A builder error fails the command with that error.
4. `task_runner.rs`: pass the builder when `orchestrate && sandbox == "on"`.
5. `orchestrator_text.rs`: `SANDBOX_ON_PARAGRAPH` says the shell and the write tools can write in
   the worktree of an open edit, until it finishes, and that `git commit` there is not possible
   (`edit-finish` commits).
6. `edit-finish` is not changed here. Report whether anything in it trusts `<wt>/.git` without
   checking it is the pointer file git wrote (on Linux the model's shell can now overwrite it).

**Tests** (`test_orchestrator_sandbox.rs`; skip only where the existing tests there skip):
- `shell_writes_in_an_open_edit_worktree_and_nowhere_else_new`: with an open edit `E1`: a write to
  `.wt/E1/f.txt` succeeds; to `.wt/E1/.git` fails (macOS); to `.wt/other/f.txt`, to `<root>/f.txt`
  and to `<root>/.git/x` fails; `git -C .wt/E1 commit` fails.
- `shell_loses_the_worktree_when_the_edit_finishes`: write `.wt/E1.json`: the next command cannot
  write in `.wt/E1`.
- `shell_keeps_its_directory_and_says_when_it_restarted`: `cd` somewhere and `export X=1`; open an
  edit; the next command prints the restart line, `pwd` is unchanged, `$X` is empty; a further
  command prints no restart line.
- `shell_does_not_restart_when_open_edits_are_unchanged`: the shell's process id (`echo $$`) is the
  same across two commands.
- Update `blyte_instructions_includes_unsandboxed_and_switches_sandbox_paragraph` only as far as
  the new paragraph wording requires.

Run: `cargo test -p blyte-agent && cargo test -p blyte --test test_orchestrator_sandbox`.

**Commit message:** `feat(sandbox): the conversation shell can write in an open edit worktree`

---

### Task SV1: every `/api/*` route of the surface needs the token

**Depends on:** none. **Siblings (not here):** DE, SH1, SH2, CO, MISC; SV2 and SV3 follow.

**Why.** Only `POST` routes check `X-Blyte-Token`; the `GET` routes (session, events, runs, map
and the rest) answer any local process or page.

**Files:** `crates/blyte-cli/src/serve.rs`, `crates/blyte-cli/assets/surface/index.html`,
`crates/blyte-cli/tests/test_serve.rs`, `crates/blyte-cli/tests/test_requests.rs`,
`crates/blyte-cli/tests/test_surface_page.rs`.

**What to build.**
1. Every route under `/api/` returns 403 without a valid token (the existing `has_valid_token`),
   whatever the method. The existing origin check stays. Unauthenticated: `/`, `/index.html`,
   `/favicon.svg`, `/fonts/*`, and one new route `GET /api/health`, which answers
   `{ "session_id": "<id>" }` and nothing else.
2. `probe_existing_surface` (`serve.rs:767-801`) uses `GET /api/health` to decide whether the
   surface recorded in `.wt/surface.json` is alive (same `session_id` comparison as today).
3. Opening a browser on a surface that is already running (`serve.rs:859`) must give the tab the
   token: on start, write the token to `.wt/surface.token` with mode `0600` (created with that
   mode, not chmod-ed afterwards) and delete it on clean shutdown along with `surface.json`; the
   second `blyte` reads it and opens `http://127.0.0.1:<port>/#token=<token>`. If the file is
   missing or unreadable, print an error naming the file and exit 1; do not open a tab without
   the token. The token never appears in `surface.json`, in logs or in any response.
4. Session id: generate `surface.session_id` before the `Session` is started and export it as
   `BLYTE_SESSION` when that variable is not already set, so ledger rows from the surface's own
   turns and from its runs carry the same id. (Task CO reads it; do not touch the ledger here.)
5. `index.html`: `fetchWithRetry` sends `X-Blyte-Token` on every `/api/*` request; a 403 on a GET
   stops all polling and shows the existing missing-token banner (no retry loop).
6. Update the `get(...)` helpers in `test_serve.rs` and `test_requests.rs` to send the token;
   that is the only change allowed to existing tests, plus assertions that named a GET answering
   without a token.

**Tests.**
- `test_serve.rs`: `every_api_get_rejects_a_missing_or_wrong_token` (list the routes from the
  router so a new route cannot be forgotten: each returns 403 without the header and with a wrong
  one, 200-class with the right one); `health_answers_without_a_token_and_gives_only_the_session_id`;
  `second_start_opens_the_page_with_the_token` (extend the existing `second_start_attaches`
  pattern: the URL handed to the opener ends `#token=<the token>`); `token_file_is_private_and_removed_on_shutdown`
  (mode `0600`; gone after shutdown); `second_start_fails_loudly_without_the_token_file`.
- `test_surface_page.rs`: `get_requests_send_the_token_and_a_403_stops_polling`.

Run: `cargo test -p blyte --test test_serve --test test_requests --test test_surface_page`.

**Commit message:** `fix(surface): every API route needs the token`

---

### Task SV2: finished runs are combined without waiting for a page request

**Depends on:** SV1 (`--base wip/SV1`). **Siblings (not here):** DE, SH1, SH2, CO, MISC; SV3 follows.

**Why.** Checks, run reports and combining only happen inside the handler for `GET /api/runs`
(`serve.rs:371-422`), so with no page open nothing is combined.

**Files:** `crates/blyte-cli/src/serve.rs`, `crates/blyte-cli/tests/test_requests.rs`.

**What to build.** Move the body that reconciles runs (ensure checks, report finished runs,
`maybe_combine`) into one function guarded by a `Mutex` on `Surface`. `handle_runs` calls it as
now. `run_with_io` also starts a thread that calls it every 2 s, and immediately when the session
goes idle if that is reachable without touching `session.rs` (otherwise the 2 s tick is enough;
say which). The thread stops when the server stops (an `Arc<AtomicBool>`, joined before
`run_with_io` returns). An error from the function on the timer is written to stderr once per
distinct message, never swallowed and never fatal to the server. Nothing may be reported or
combined twice: the mutex covers the whole function.

**Tests** (`test_requests.rs`):
- `a_ready_request_is_combined_with_no_page_request`: set up a ready, accepted request as the
  existing combine tests do; make no `GET /api/runs`; within 10 s `.wt/combined/<request>.json`
  exists.
- `timer_and_page_requests_do_not_report_a_run_twice`: hammer `GET /api/runs` from two threads
  while the timer runs; the session has exactly one report entry for the run.
- `the_timer_thread_ends_with_the_server`: after the server stops, a request made ready afterwards
  is not combined within 5 s.
- The existing five combine tests stay unmodified and passing.

Run: `cargo test -p blyte --test test_requests --test test_serve`.

**Commit message:** `fix(surface): combine finished runs on a timer, not only when the page asks`

---

### Task SV3: the surface keeps its entries across a restart

**Depends on:** SV2 (`--base wip/SV2`). **Siblings (not here):** DE, SH1, SH2, CO, MISC.

**Why.** The conversation entries the page shows live only in memory; restarting `blyte` shows an
empty page although the runs and requests are still on disk.

**Files:** `crates/blyte-cli/src/session.rs`, `crates/blyte-cli/src/serve.rs`,
`crates/blyte-cli/tests/test_session.rs`, `crates/blyte-cli/tests/test_serve.rs`.

**What to build.**
1. `Entry` and `EntryBody` derive `Deserialize`. `Session` takes an optional log path; every entry
   pushed is appended to it as one JSON line, flushed before the entry is visible to readers.
2. The surface uses `.wt/sessions/entries.jsonl` in the main checkout (one file per repository;
   `.wt/` is git-ignored). On start, if it exists, its entries are loaded in order, `last_seq`
   continues from the last one, and one new entry is appended whose text is exactly
   `blyte restarted: the entries above are from before; the model does not remember them`, of
   the same kind the session uses for its own notices. Only the entries are restored, not the
   model's state.
3. A last line cut short by a crash (not valid JSON, and the final line) is dropped with a line on
   stderr naming the file and the line number. An invalid line anywhere else is an error that
   names the file and line and stops the start: no silent skipping of the middle of a log.
4. Approval requests that were waiting when the process stopped are loaded as entries but are not
   live: they must not block or be answerable. Say in the report how you represent that.
5. No cap or rotation in this task; say the file's size per 100 entries in the report.

**Tests.**
- `test_session.rs`: `entries_are_written_as_they_are_pushed` (the line is on disk before
  `entries_after` returns it); `a_restart_loads_entries_and_continues_numbering` (no gap, no
  repeat, the restart notice is last); `a_cut_off_last_line_is_dropped_and_reported`;
  `a_bad_line_in_the_middle_stops_the_start_naming_the_line`;
  `a_loaded_approval_request_is_not_live`.
- `test_serve.rs`: `events_survive_a_restart` (start, send a message, stop, start again in the
  same repository: `GET /api/session/events` returns the earlier entries then the notice).
- Existing tests in both files stay unmodified and passing.

Run: `cargo test -p blyte --test test_session --test test_serve`.

**Commit message:** `feat(surface): keep the session's entries across a restart`

---

### Task CO: `blyte cost` by session, with the model review's cost in the ledger

**Depends on:** none. **Siblings (not here):** DE, SH1, SH2, SV1–SV3 (SV1 makes the surface export
its session id as `BLYTE_SESSION`; do not touch `serve.rs`), MISC (`usage_log.rs`; do not change it).

**Why.** `blyte cost` picks rows by repository and time, so sessions running at the same time in
one repository are added together; and the fresh model review calls the provider
(`model_review.rs`, about lines 660 and 683) without writing its usage to the ledger.

**Files:** `crates/blyte-cli/src/model_review.rs`, `crates/blyte-cli/src/cost.rs`,
`crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/tests/test_cost.rs`,
`crates/blyte-cli/tests/test_fresh_model_review.rs`. If `main.rs` must pass a new argument
through, that one call site may change too.

**What to build.**
1. After each provider call in the model review (the first attempt and the retry), append one
   ledger row with `usage_log::append`: `outcome: "model_review"`, `dispatch: Some(<run id>)`, the
   model, the tokens and cost of that call, the repository, and `session` = `BLYTE_SESSION` when
   set, else the `session` in `.wt/<id>.start.json`, else none (as other rows do when there is no
   session). A row is written for a call that returned an unusable answer too: it was paid for.
2. `cost::totals`: a `model_review` row adds to cost and tokens but not to the count of runs
   (as `turn` rows already do not).
3. `blyte cost --session <id>`: only rows with that session id. `blyte cost --by-session`: one line
   per session id (rows without one under `(none)`), with first and last time, runs, turns, model
   reviews and cost, ordered by first time, then the existing total. Both combine with `--repo`
   and `--since`, and with `--json` (an array `sessions` of objects with the same fields). Costs
   are printed as they are today (USD).
4. An unknown `--session` id prints the totals of zero rows and exits 0, as an unmatched `--repo`
   does today; say so in the help text.

**Tests.**
- `test_fresh_model_review.rs`: `a_review_writes_its_cost_to_the_ledger` (one row,
  `outcome: "model_review"`, the run id, the fake provider's cost, the session from
  `BLYTE_SESSION`); `a_retried_review_writes_two_rows_that_sum_to_its_cost`;
  `the_session_comes_from_the_start_record_when_the_environment_has_none`.
- `test_cost.rs`: `model_review_rows_add_cost_but_not_runs`; `session_filter_selects_only_that_session`
  (two sessions interleaved in time in one repository); `by_session_lists_each_session_and_the_total`
  (text and `--json`); `rows_without_a_session_are_listed_as_none`. The existing
  `old_ledger_totals_unchanged` stays unmodified and passing.

Run: `cargo test -p blyte --test test_cost --test test_fresh_model_review --test test_usage_session && cargo test -p blyte --lib cost`.

**Commit message:** `feat(cost): report cost by session and record the model review's cost`

---

### Task MISC: tests cannot reach the real usage ledger; the old name leaves the terminal

**Depends on:** none. **Siblings (not here):** DE, SH1, SH2, SV1–SV3, CO.

**Files:** `.cargo/config.toml`, `crates/blyte-cli/src/usage_log.rs`, `crates/blyte-cli/src/repl.rs`,
`crates/blyte-cli/tests/test_e2e_cli.rs`.

**What to build.**
1. `.cargo/config.toml` `[env]`: add `BLYTE_USAGE_LOG = "off"` with a comment in the style of the
   two entries there (tests never write the developer's usage ledger; a test that needs a ledger
   points this at a temporary file). In `usage_log.rs` add the unit test
   `the_ledger_is_off_under_cargo_test` (`default_path()` is `None` in the test environment). If
   an existing test fails because it relied on the real path, do not weaken it: point it at a
   temporary directory and name it in the report.
2. `repl.rs`: replace the ASCII-art banner (about lines 316-336) with the single line `blyte`
   followed by the version line that is printed today; replace `=== Blyte SKILL.state ===` (lines
   415 and 564) with `=== blyte state ===`. Search the crate's user-visible strings for any other
   old product name and list what you find in the report; change only those in `repl.rs`.
3. `test_e2e_cli.rs` line 72: the `/state` header assertion follows the new text. That is the only
   existing assertion that may change.

**Tests.** `the_ledger_is_off_under_cargo_test` above; in `test_e2e_cli.rs`,
`the_banner_is_one_line_with_the_name` (the interactive start prints a line that is exactly
`blyte` and no line with five or more consecutive `_`, `|`, `/` or `\` characters).

Run: `cargo test -p blyte --test test_e2e_cli --test test_orchestrator_instructions --test test_orchestrator_sandbox && cargo test -p blyte --lib usage_log`.

**Commit message:** `chore: keep tests off the real usage ledger; drop the old name from the terminal`

---

### Task SH3: `edit-finish` checks the worktree's `.git` before running git in it

**Depends on:** DE and SH2 (both on the branch this is dispatched from). **Siblings (not here):**
SV2, SV3 (`serve.rs`, `session.rs`).

**Why.** Since SH2 the model's sandboxed shell can write in an open edit worktree. On macOS the
sandbox denies writes to `<wt>/.git`; on Linux Landlock cannot deny one file inside a writable
directory, so the shell could replace `.wt/<id>/.git` (a pointer to another repository, or a
directory with its own `config` and hooks). `edit-finish` then runs git there outside the sandbox.

**Files:** `crates/blyte-cli/src/direct_edit.rs`, `crates/blyte-cli/tests/test_direct_edit.rs`.

**What to build.** In `run_edit_finish_in`, after the check that the worktree directory exists and
before the first git command or gate step runs in it, call a new
`fn verify_worktree_git_pointer(main_checkout: &Path, worktree_dir: &Path, id: &str) -> Result<(), String>`:
- `<wt>/.git` must be a regular file, not a symlink and not a directory (`symlink_metadata`);
- its whole content, trimmed of one trailing newline, must be exactly `gitdir: <P>`, where `<P>`
  canonicalizes to the canonical `<main checkout's git common dir>/worktrees/<id>`;
- that directory must exist and its `gitdir` file must point back at `<wt>/.git` (canonical paths).
On any mismatch `edit-finish` prints an error that names `<wt>/.git` and what was wrong, runs no
git command and no gate step in the worktree, leaves the edit open and exits 2. No fallbacks: do
not repair the file and do not continue. This is a check on every platform, not Linux only.

**Tests** (`test_direct_edit.rs`; existing tests unmodified and passing):
- `finish_refuses_a_git_pointer_to_another_repository`: overwrite `.wt/<id>/.git` with
  `gitdir: <another repo>/.git`; a gate that writes a marker file; exit 2, the error names the
  file, the marker does not exist, `.wt/<id>.edit.json` exists and `.wt/<id>.json` does not.
- `finish_refuses_a_git_directory`: replace the file with a directory holding a `config`; same
  assertions.
- `finish_refuses_a_symlinked_git_pointer`: replace it with a symlink to a copy of the original
  pointer file; same assertions.
- `finish_accepts_the_pointer_git_wrote`: an untouched edit finishes as before.

Run: `cargo test -p blyte --test test_direct_edit`.

**Commit message:** `fix(edit): edit-finish refuses a worktree whose .git is not the pointer git wrote`
