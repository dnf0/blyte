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
