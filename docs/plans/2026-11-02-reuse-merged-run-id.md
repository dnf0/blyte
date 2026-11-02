# Plan: reusing the ID of a merged run replaces that run (2026-11-02)

## Why

Dispatching a task under an ID that an earlier run used fails when `wip/<ID>` or `.wt/<ID>` is still there
(`branch_exists_message`, `crates/blyte-cli/src/dispatch.rs:130`; the older-base error, about line 570). Plans reuse short
IDs (T1, SC1), so this keeps happening. Each time, the caller has to rename the tasks and re-dispatch. In one repository
(2026-11-02), `wip/SC1`..`SC5` and `.wt/SC1`..`SC5` were from 2026-10-15. `blyte clean --dry-run` reports all five as
merged (two by cherry-pick record, three by squash subject match), yet they were never removed:

- the automatic clean at dispatch (`clean_quiet`, dispatch.rs about line 393) skips the ID being dispatched
  (`skip_id`) on purpose;
- it stops after 1 s, and with about 60 runs in that repository it never reaches the S's.

Decision: when the earlier run under the same ID is finished and its work is merged (or it made no commits), dispatch
removes it, as `blyte clean` would, moves its records aside, and goes on. An earlier run that is not merged still stops
the dispatch with today's message: its work may be needed, and only the user can decide.

### Task RU1: dispatch replaces a merged earlier run with the same ID

Files: `crates/blyte-cli/src/clean.rs`, `crates/blyte-cli/src/dispatch.rs`; new tests in
`crates/blyte-cli/tests/test_dispatch_reuse_merged.rs` (follow the repository setup in `test_dispatch_branch_exists.rs` and
`test_dispatch_reuse.rs`). Grep first; never read `dispatch.rs` or `clean.rs` whole.

No fallbacks: every git or file operation that fails stops the dispatch with an error naming the command or path. Never
treat "could not tell whether it is merged" as merged.

1. In `clean.rs`, add `pub fn remove_finished_run(root: &Path, id: &str) -> Result<Option<String>, String>`, using
   `CleanContext` and the same checks `clean_repo` uses for one ID:
   - If `.wt/<id>` exists: it is removable only when `check_removable(id, u64::MAX)` returns `Merged`, `MergedInto(_)`,
     `MergedThrough(_)` or `NoChanges` (not `Verdict`, `OlderThan` or `ResearchReport`: those can hold unmerged work), and
     `has_uncommitted_changes` is false. Then `git worktree remove .wt/<id>` (no `--force`), and delete `wip/<id>` with
     `git branch -D` if it exists.
   - If only `wip/<id>` exists (no worktree): removable when `is_branch_merged(root, "wip/<id>")`. Then `git branch -D`.
   - Returns `Ok(Some(reason))` (the `RemovalReason` text, e.g. "merged into main") when it removed the run, `Ok(None)` when
     the run is not removable (leave everything as it was), and `Err(..)` naming the failed command and its stderr.
   - Factor shared code out of `clean_repo` rather than copying it, if that is a small change; do not change what
     `blyte clean` removes.
2. In `dispatch.rs`, for a non-research dispatch, after the "still running" check (about line 386) and before
   `clean_quiet`: when `.wt/<id>` or `wip/<id>` exists, call `remove_finished_run`.
   - `Ok(Some(reason))`: move the earlier run's records `.wt/<id>.json`, `.wt/<id>.report.json`, `.wt/<id>.start.json`,
     `.wt/<id>.verdict.json` and `.wt/<id>.log` (each only if it exists) into a new directory
     `.wt/archive/<id>-<unix seconds>/`. Do not touch any other `.wt/<id>.*` file (the caller may already have written
     `.wt/<id>.task.md` or be writing `.wt/<id>.out` for this dispatch). Print to stderr one line:
     `dispatch: replaced the earlier run <id> (<reason>); its records are in .wt/archive/<id>-<secs>/`. Then continue as
     a fresh dispatch.
   - `Ok(None)`: go on to today's checks unchanged, so the existing messages (branch exists and is not merged, older
     base, and so on) still appear.
   - `Err(e)`: print `error: could not remove the earlier run <id>: <e>` and return 2.
   - Research dispatches are unchanged.
   - The new `.start.json` for this dispatch must be written after the move (check where it is written).
3. Make sure nothing that scans `.wt/` mistakes `.wt/archive` for a run: it has no `archive.start.json`, so
   `find_blyte_worktrees` and the `*.start.json` listings skip it. Check `blyte clean`, `blyte review` and the usage report
   code paths with grep, and add a test if any of them lists directories without that check.
4. Tests (real git repositories in temp dirs, a fake provider or `--dry-run`-style setup as the existing dispatch tests do):
   - An earlier run whose `wip/<id>` commit is merged into the base by cherry-pick with `-x`: dispatch with the same ID
     succeeds, prints the "replaced the earlier run" line, the old records are in `.wt/archive/<id>-*/`, and the new run's
     worktree is based on the new base.
   - Same with a squash merge (base has one commit whose message lists the run's commit subject).
   - An earlier run with commits that are not merged: dispatch fails with today's "is not merged" message, and the
     worktree, branch and records are untouched.
   - A merged earlier run whose worktree has uncommitted changes: dispatch fails, nothing is removed.
   - Only the branch is left (no worktree), merged: it is deleted and dispatch proceeds.
   - `.wt/<id>.task.md` written before the dispatch is still in place afterwards.

Gate: the repository checks. Commit: `fix(dispatch): reusing a merged run's ID replaces that run instead of failing`
