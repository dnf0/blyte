# `blyte review` on a research run whose scratch folder still exists

`blyte review <id>` fails on a research run while its scratch folder `.wt/<id>` is still there:

```
error: cannot find where run DCR1 started; its start record has no base_sha and the worktree shares no history with <sha>
```

Seen on 2026-11-27 on a research run that ended `budget_exhausted` (a verified research run has its
scratch folder removed at once, so the fault does not show there). Once the folder is removed, the
same command works.

Cause: `blyte dispatch --research` makes `.wt/<id>` a git repository of its own (`git init`, then one
empty commit with the message `research`; see `crates/blyte-cli/src/dispatch.rs`, the `args.research`
branch that creates the scratch directory). Its start record has `"base_sha": null`.
`read_worktree_git_data` in `crates/blyte-cli/src/queue_actions.rs` then falls back to
`git merge-base HEAD <the main repository's HEAD>` run inside the scratch repository, which can never
succeed: the two repositories share no history.

### Task RV1: review reads a research run's scratch repository from its own first commit

**Depends on:** none. **Siblings:** none.

**Files:** `crates/blyte-cli/src/queue_actions.rs`, `crates/blyte-cli/tests/test_queue_actions.rs`.

**What to build.** In `read_worktree_git_data`, when the run is a research run (`is_research`) and
its start record has no `base_sha`, the base is the first commit of the scratch repository itself:
the output of `git rev-list --max-parents=0 HEAD` run in `wt` through `run_git`. Do not run
`git rev-parse HEAD` or `git merge-base` against `root` for such a run.

- `WtGit::WorktreeGone` returns `Ok(None)`, as the other git calls in this function do.
- `WtGit::IoErr(e)` returns an error that names the command and `wt`.
- If the command fails, or prints no commit, or prints more than one line, return an error that
  says so and names the run and `wt`, for example
  `cannot find the first commit of research run {id} in {wt}: {stderr or what was printed}`.
  **No fallbacks**: do not substitute `HEAD`, an empty range, or the main repository's history.
- Everything after the base is found stays as it is (`{base}..HEAD` for commits, files, and the diff
  when asked for).
- A run that is not a research run, and a research run that does have a `base_sha`, behave exactly
  as before. Do not change the existing error text for those.

Do not change `dispatch.rs` or the start record.

**Tests**, in `crates/blyte-cli/tests/test_queue_actions.rs`, following the way that file already
builds a repository and a run (read its existing research tests first and reuse its helpers):

1. `review_of_a_research_run_with_its_scratch_folder_present_succeeds`: a repository `root` with one
   commit; `.wt/R1` made the way dispatch makes it (`git init -q`, then
   `git -c user.name=blyte -c user.email=blyte@local commit -q --allow-empty -m research`), holding
   an untracked `report.md`; `.wt/R1.start.json` with `"research": true` and `"base_sha": null`;
   `.wt/R1.json` with `"outcome": "budget_exhausted"` and the other fields the review needs;
   `.wt/R1.report.md`. The review computation succeeds, lists no commits, and its tree section
   mentions `report.md`. Before the change this test fails with "cannot find where run R1 started".
2. `review_of_a_research_run_shows_commits_made_in_its_scratch_folder`: the same, plus one commit in
   `.wt/R1` after the first that adds `notes.md`; the review lists that commit and `notes.md`.
3. `review_of_a_research_run_whose_scratch_repository_has_no_commit_errors_naming_it`: `.wt/R1` with
   `git init -q` only. The review returns an error containing `research run R1` and the path of
   `.wt/R1`.
4. The existing test that expects "shares no history" for a run that is not research (if there is
   one) passes unchanged; if there is none, add one: a non-research run with no `base_sha` whose
   worktree is an unrelated repository still gets the existing error.

Run them with `cargo test -p blyte --test test_queue_actions`.

**Constraints.** Existing tests pass unmodified. No fallbacks. Touch only the two files.

**Commit message:** `fix(review): a research run is reviewed while its scratch folder still exists`

### Task ST1: the surface records and shows how long each reply took

**Depends on:** none. **Siblings:** RV1 changes `queue_actions.rs` separately; it is not in this
worktree and is not to be touched here.

**Why.** On 2026-11-27 three short replies in a surface took 11 s, 8 s and 18 s (91, 148 and 426
output tokens). The log has only whole seconds and no time of first output, so nobody (the user or
the model) can say where the time went.

**Files:** `crates/blyte-cli/src/session.rs`, `crates/blyte-cli/assets/surface/index.html`,
`crates/blyte-cli/tests/test_session.rs` (and `crates/blyte-cli/src/serve.rs` only if an
exhaustive match there needs the new fields).

**What to build.**

1. The `Idle` entry (`EntryBody::Idle` and `EntryBodyWire::Idle` in `session.rs`) gains two fields:
   - `seconds: Option<f64>`: from the moment the user's message began to be handled (the point the
     user-message entry for this reply was pushed) to the moment the `Idle` entry is pushed,
     measured with `std::time::Instant`, rounded to one decimal place;
   - `first_output_seconds: Option<f64>`: from the same start to the first model output of that
     reply (the first text or thinking delta, or the first tool call, whichever comes first),
     rounded to one decimal place; `None` when the reply produced no model output.
   Both are `#[serde(default, skip_serializing_if = "Option::is_none")]` on the wire, so a log
   written before this change still reads; an old entry reads as `None`. `None` means "not
   measured", and nothing may show it as zero. **No fallbacks**: do not estimate either value from
   the whole-second `ts` fields.
2. In `assets/surface/index.html`, the line under a finished reply (now
   `N turns · USD 0.02`) becomes `N turns · USD 0.02 · 8.4 s (first output 6.1 s)` when both are
   present, `· 8.4 s` when only `seconds` is, and is unchanged when neither is.

**Tests** in `crates/blyte-cli/tests/test_session.rs`, using the scripted model the existing tests
there use (read them first):

1. `idle_entry_records_how_long_the_reply_took`: a scripted reply; the `Idle` entry has
   `seconds` `Some(s)` with `s >= 0.0`, and `first_output_seconds` `Some(f)` with `f <= s`.
2. `idle_entry_without_times_still_reads`: the JSON line
   `{"seq":1,"ts":1,"kind":"idle","turns":1,"cost_usd":0.01}` deserialises, with both fields `None`,
   and serialises back without either key.
3. `surface_page_shows_reply_time`: `assets/surface/index.html` contains `first_output_seconds`
   and the text `first output`.

Run with `cargo test -p blyte --test test_session`.

**Constraints.** Existing tests pass unmodified, apart from adding the two fields where a test
builds an `Idle` entry by hand. No fallbacks.

**Commit message:** `feat(surface): each reply shows how long it took`

### Task AT1: model activity moves out of the chat into an Activity tab

**Depends on:** ST1 (same file). **Siblings:** none running.

**Why.** In the Design view every tool call (`bash ls …  exit 0`, `read …`) and every
`conversation model · turn N` label is drawn in the chat, so one reply that runs ten commands
pushes the conversation off the screen. The chat should hold the conversation; what the model did
belongs on its own screen, with only a summary in the chat.

**Files:** `crates/blyte-cli/assets/surface/index.html`, `crates/blyte-cli/tests/test_surface_page.rs`.
No server change: the page already receives every entry.

**What to build.**

1. A third tab button `id="mode-activity"`, text `Activity`, after `mode-design` and
   `mode-review` in the same tab list, with the same classes and `role="tab"`. The mode becomes one
   of `design`, `review`, `activity`; `renderModeChrome` shows exactly one of three panes and sets
   `active` / `aria-selected` on exactly one tab. The new pane is `id="activity"`.
2. **Activity pane.** Everything the chat drew for the model's work is drawn here instead, in
   order, grouped under one heading per user message (the first 80 characters of the message, with
   its time): the `conversation model · turn N` labels, tool calls with their status
   (`exit 0`, errors, "still waiting"), and thinking, if the page draws it today. Reuse the existing
   drawing code and classes; move, do not duplicate.
3. **Chat (Design view) keeps only:** the user's messages; the model's text replies; approval
   requests and their answers (the user must act on them, so they stay in the chat, exactly as
   now); errors that end a reply; the no-repository notice; and the line under a finished reply
   (turns, cost, time).
4. **Summary line.** Where a reply ran at least one tool call, the chat shows one line for that
   reply, placed before the reply's text, of class `activity-summary`:
   `3 commands · 1 file read · 1 failed`, built by counting that reply's tool entries by tool name
   (`bash` → commands; `read` → files read; `write`/`edit` → files changed; every other tool by its
   own name, e.g. `2 blyte`), plus `N failed` when any had an error; parts with a count of zero are
   left out. It is a button: clicking it switches to the Activity tab and scrolls to that reply's
   heading. While the reply is still running the same line reads `working · <the latest tool call,
   one line, truncated>` and updates in place.
5. A tool name the page does not know is counted under its own name. **No fallbacks**: do not drop
   an entry kind from both views; any entry kind the chat stops drawing must be drawn in Activity.

**Tests** in `crates/blyte-cli/tests/test_surface_page.rs`, in the style already used there (read
it first):

1. `surface_page_has_an_activity_tab`: the page contains `id="mode-activity"` and `id="activity"`.
2. `surface_page_summarises_activity_in_the_chat`: the page contains `activity-summary`.
3. If that file has a way to run the page's script functions (for example under `node`), add a test
   that a reply with two `bash` calls, one `read` and one failed call yields
   `2 commands · 1 file read · 1 failed`; if it has none, keep the summary-building code in one
   function named `activitySummaryText(entries)` and test that the page defines it.

Run with `cargo test -p blyte --test test_surface_page`.

**Constraints.** Existing tests pass; a test that asserts tool calls are drawn in the chat may be
changed to assert they are drawn in Activity, and say so in the report. Touch only the two files.

**Commit message:** `feat(surface): model activity has its own tab; the chat shows a summary`
