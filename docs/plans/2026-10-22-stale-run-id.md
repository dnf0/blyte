# Reused run IDs: stale worktrees and deleted running runs

Date: 2026-10-22.

## Problem

Two failures seen on 2026-10-22, both from dispatching an ID that an
earlier run had used:

1. **A stale worktree is reused.** `blyte dispatch S1` found `.wt/S1` from a run on 2026-10-20,
   whose work had since been squash-merged. `run_dispatch` only checks for an existing
   `wip/<ID>` branch inside `else if !worktree_dir.exists()` (`dispatch.rs`), so an existing
   worktree is reused as it is, on its old base. The dispatch-time `clean_quiet(&root,
   Some(&args.id))` skips the ID being dispatched, so it never removes it.
2. **A running run's worktree is deleted.** `S2` was dispatched while `.wt/S2.json` from a run on
   2026-09-29 was still on disk (dispatch removes only the old report files). A later dispatch's
   `clean_quiet` ran `check_removable("S2")`: because a summary existed, it skipped the live-PID
   check (`clean.rs`, "Check if run is finished"), saw `wip/S2` with no commits of its own as
   merged, and removed `.wt/S2` while the run (PID alive) was on turn 22.

## Decisions

- A run counts as running when the PID in `.wt/<ID>.start.json` is alive **and** there is no
  summary written since that start: no `.wt/<ID>.json`, or one whose modification time is earlier
  than the start file's `started_at`. A summary written at or after `started_at` means this run
  finished and the PID may have been reused, as now.
- Re-dispatching an ID whose worktree exists stays allowed (it is how a failed run is retried),
  but only when the worktree's `HEAD` already contains the commit the new dispatch is based on.
  Otherwise dispatch refuses with an error naming both commits. It never resets or deletes the
  old worktree itself.
- Re-dispatching an ID whose previous run is still running is refused.
- A new fix run deletes the earlier run's `.wt/<ID>.json` when it starts, so status, dashboard and
  clean do not read the old outcome while the new run is going. Verdict files are kept.
- No fallbacks: an unreadable start file, timestamp or `git` result is an error that names what
  is missing, not "not running" or "reuse".

### Task SR1

**Goal:** `check_removable` must never remove the worktree of a run whose process is alive just
because an older summary file exists.

**Files:** `crates/blyte-cli/src/clean.rs`; new test file
`crates/blyte-cli/tests/test_clean_running_stale_summary.rs`.

**What to build:**
- In `check_removable` (the block commented "Check if run is finished"), replace the
  `if !summary_exists { ... }` guard with: if the start file has a `pid` and `is_pid_alive(pid)`,
  and either the summary file does not exist or its modification time (seconds since the Unix
  epoch) is earlier than the start file's `started_at` (use `parse_started_at`), return
  `Err(KeptReason::Running)`. This check stays first, before every removal rule (redone verdict,
  `no_changes`, merged, age).
- When the PID is alive, a summary exists, and `started_at` is missing or unparsable, return
  `Err(KeptReason::Running)` too: with a live PID and no way to tell the runs apart, keeping the
  worktree is the only safe answer. Do not add any other default.
- Everything else in `check_removable` is unchanged.

**Tests** (in the new file, built like `tests/test_clean_quiet.rs`: a temp git repo, a
`wip/<ID>` branch with no commits of its own, a worktree at `.wt/<ID>`, and a live PID from a
child `sleep 60` process that the test kills at the end):
1. Live PID, start file `started_at` = now, summary `.wt/<ID>.json` with mtime one hour before
   (set with `filetime` if it is already a dev-dependency, otherwise by writing the file and
   setting the time with `touch -t` through `std::process::Command`): `blyte clean` (and
   `clean_quiet`) keeps `.wt/<ID>`.
2. Same, plus `.wt/<ID>.verdict.json` with verdict `redone`: still kept.
3. Live PID, summary mtime after `started_at`: removed, as before (a finished run whose PID
   was reused).
4. Dead PID (999999), stale summary: removed, as before.
5. Live PID, summary exists, start file has no `started_at`: kept.

Run: `cargo test -p blyte-cli --test test_clean_running_stale_summary`, then the full gate.

**Constraints:** existing tests pass unmodified. No fallbacks.

**Depends on:** none.

**Siblings:** SR2 changes `dispatch.rs` separately (worktree reuse and deleting the old summary);
it is not in this worktree. Do not change `dispatch.rs`.

**Commit message:** `fix(clean): keep a running run's worktree when an older summary exists`

### Task SR2

**Goal:** `blyte dispatch <ID>` must not reuse a worktree left from an earlier run on an older
base, must not start while a run with that ID is still going, and must clear the earlier run's
summary when it starts.

**Files:** `crates/blyte-cli/src/dispatch.rs`; new test file
`crates/blyte-cli/tests/test_dispatch_reuse.rs`.

**What to build** (fix runs only; research runs keep their current "already exists" error):
- In `run_dispatch`, before the worktree is created or reused (and before `clean_quiet`), when
  `.wt/<ID>.start.json` exists and its `pid` is alive (`crate::clean::is_pid_alive`) and there is
  no `.wt/<ID>.json` written at or after its `started_at`, exit 2 with:
  `error: run <ID> is still running (pid <PID>). Wait for it to finish, or dispatch with a new ID.`
  An unreadable or unparsable start file is an error naming the file, exit 2.
- When `.wt/<ID>` exists, resolve the commit the new dispatch is based on (the same commit a
  fresh worktree would be created from: `--base` if given, else `HEAD` of the main checkout) and
  the worktree's `HEAD`. If the base commit is not an ancestor of the worktree's `HEAD`
  (`git merge-base --is-ancestor <base> <worktree HEAD>`, exit 1), exit 2 with:
  `error: .wt/<ID> is from an earlier run based on <short worktree HEAD>, which does not contain <short base> (<base name>). Run `blyte clean`, remove it with `git worktree remove .wt/<ID>` and `git branch -D wip/<ID>` if it is not needed, or dispatch with a new ID.`
  Any other `git` failure is an error naming the command, exit 2. If it is an ancestor, reuse
  the worktree as today.
- Once the run is going to start (after the checks above pass), delete `.wt/<ID>.json` if it
  exists, next to the existing removal of the old report files. Keep `.wt/<ID>.verdict.json`.

**Tests** (new file, built like `tests/test_dispatch_branch_exists.rs`, using `--gate true
--sandbox off` and no API key, so runs end quickly):
1. Dispatch `T1`, then commit a new file on `main`, then dispatch `T1` again: exit 2, stderr
   contains `error: .wt/T1 is from an earlier run based on` and `does not contain`; the worktree
   still exists and its `HEAD` is unchanged.
2. Dispatch `T1` twice with no new commit on `main`: the second dispatch reuses the worktree
   (exit code as in `test_dispatch.rs`'s "reuses the worktree" case, not 2).
3. Write `.wt/T1.start.json` with the test's own PID (`std::process::id()`) and `started_at` = now,
   and no `.wt/T1.json`: dispatch `T1` exits 2 with `error: run T1 is still running (pid `.
4. Write a stale `.wt/T1.json` containing `{"outcome":"verified"}` before the first dispatch of
   `T1`: after the dispatch, `.wt/T1.json` holds this run's outcome, not `verified`; a
   pre-existing `.wt/T1.verdict.json` is still there.
5. `.wt/T1.start.json` containing `not json`: exit 2 and stderr names the file.

Run: `cargo test -p blyte-cli --test test_dispatch_reuse`, then the full gate.

**Constraints:** existing tests pass unmodified, including the reuse case in
`tests/test_dispatch.rs`. No fallbacks.

**Depends on:** none.

**Siblings:** SR1 changes `check_removable` in `clean.rs` separately; it is not in this worktree.
Do not change `clean.rs`.

**Commit message:** `fix(dispatch): refuse to reuse a worktree on an older base or a running ID`
