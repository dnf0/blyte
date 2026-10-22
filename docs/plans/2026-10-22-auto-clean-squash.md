# Plan: automatic cleanup that sees squash merges (2026-10-22)

## Why

A user ran out of disk space on 2026-10-22. Their team squash-merges PRs on GitHub. The automatic
cleanup ran at the start of every dispatch (`clean_quiet`) and on `blyte review --verdict`
(`clean_on_review`), but used only the cheap checks: ancestor, cherry-pick record, and follow-ups
through those. A squash merge makes new commits, so every finished run was kept as "not merged yet"
until the 14-day age rule, and each worktree held its own `.venv` or `target/`. The squash-subject
check from 0.35.1 (#299, `is_squash_merged_into_target`) ran only in an explicit `blyte clean`.

Decisions (user, 2026-10-22):
1. The quick clean also runs the squash-subject check. It is three git commands per run, cheap enough
   for the 1-second limit.
2. `blyte review` cleans the repository too, not only the run being reviewed. The verdict is recorded
   before that run is merged, so `clean_on_review` alone can never see the merge. Reviewing the next
   run then sweeps the runs merged since.

Verified 2026-10-22 in `crates/blyte-cli/src/clean.rs`:
- `decide_run_merged` (line 841):
  - Steps 5-8 come after `if self.quiet { ... }` (about line 904), so a quiet clean returns before step 8, the squash check (about lines 961-975).
  - In step 3, the base-branch cherry and merge-tree checks are skipped when `self.quiet`.
- `clean_repo` (about line 1545) stops a quiet clean after 1 s.
- `clean_quiet(root, skip_id)` is at line 1971.
- `clean_on_review(root, id)` is at line 1986. It only handles `id`, and prints `worktree .wt/{id} kept until wip/{id} is merged`.
- Merge checks compare against the local default branch and `origin/<default>`, so a squash merge on GitHub is seen only after a `git fetch` or `git pull`.
  The blyte-implementer flow pulls main after merging. This plan does not add a fetch.

### Task CL1: squash check in the quick clean, and a repository clean on review

Files: `crates/blyte-cli/src/clean.rs` (only `decide_run_merged`, `clean_on_review`, and
`clean_quiet` if needed), new test `crates/blyte-cli/tests/test_clean_quiet_squash.rs`, and
`docs/usage.md` (the "Cleaning finished worktrees" section: the step 8 line with
"(explicit clean only)", about line 300; and the paragraph starting "Before creating a new
worktree", about line 310).

**`clean.rs` is about 2,000 lines: never read it whole.** Read `decide_run_merged` (lines 841-990)
and `clean_on_review` (1986-2045). Use `grep -n` for anything else. No fallbacks: keep every
existing rule, and do not count a run as merged on any new grounds other than the squash check.

1. **Squash check in quiet mode.** In `decide_run_merged`, inside the `if self.quiet { ... }` early return:
   before it returns, run the step 8 squash-subject checks against the default branch, `origin/<default>`
   and the base targets, with the same result values as step 8.
   - Move the step 8 block into a small helper, `fn squash_merged_reason(&self, tip_sha, base_targets) -> Option<RemovalReason>` (or the existing return type),
     so it is written once and called from both places.
   - Keep step 8 itself in the explicit path. It is not reached twice, because quiet mode returns first.
2. **Review cleans the repository.** At the end of `clean_on_review`, on every path that returns after
   handling `id` (kept or removed), call `clean_quiet(root, Some(id))`. It prints
   `blyte: removed N finished worktrees (X GB)` only when it removed something.
   - Skip `id`: its own case has just been handled, and repeating it would print twice.
   - The early `return` when `.wt/{id}` does not exist should also run `clean_quiet(root, Some(id))`.
3. **Tests** (`test_clean_quiet_squash.rs`):
   - Copy the `git`, `write_file` and `init_repo` helpers and the squash-merge setup from `crates/blyte-cli/tests/test_clean_squash.rs`.
   - Copy how a run is laid out (`.wt/<id>` worktree, `.wt/<id>.start.json`, `.wt/<id>.json`, branch `wip/<id>`) from `crates/blyte-cli/tests/test_clean_quiet.rs`.
   - Cases:
     - **Squash merged:** a finished run `A1`, whose `wip/A1` commits are listed in a squash commit on main, followed by a later commit on main that edits the same lines.
       `clean_quiet(root, None)` removes `.wt/A1`, and `wip/A1` is deleted. This test must fail without step 1.
     - **Partly listed:** the same, but the squash body lists only one of A1's two commit subjects. `clean_quiet` keeps `.wt/A1`.
     - **Review sweep:** runs `A1` (squash merged as above) and `B1` (finished, not merged).
       `clean_on_review(root, "B1")` keeps `.wt/B1` and removes `.wt/A1`.
4. **Docs** (`usage.md`):
   - Drop "(explicit clean only)" from step 8.
   - In the "Before creating a new worktree" paragraph, add the squash-subject check to the list of cheap checks.
   - Add: `blyte review --verdict` runs the same quick clean over the repository after handling the reviewed run. A squash merge on GitHub is seen once the default branch has been fetched or pulled.

Gate: the repository checks. Commit: `fix(clean): see squash merges in the automatic clean, and sweep the repository on review`
