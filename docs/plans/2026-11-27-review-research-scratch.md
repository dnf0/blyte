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
