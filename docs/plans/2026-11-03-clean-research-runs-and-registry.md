# Clean finished research runs; forget repositories that no longer exist

## Problem

`blyte clean --all --dry-run` on 2026-11-03 kept 17 research runs as "no report". They fall in
three groups:

1. **Ended without a report** (8 runs): the summary `.wt/<id>.json` exists, its `outcome` is
   `error` or `budget_exhausted`, and no `report.md` was written. The worktree holds nothing:
   research runs don't commit. The summary and log in `.wt/` are what is worth keeping. Today these
   are only removed after 14 days.
2. **Dispatch stopped, report written** (7 runs, 2026-10-06 to 2026-10-21): no summary, not
   running, and `.wt/<id>/report.md` exists but was never copied out to `.wt/<id>.report.md`. The
   dispatch process ended before the copy (for example the session closed).
3. **Dispatch stopped, no report** (2 runs, 2026-10-19 and 2026-10-22): no summary, not running,
   no `report.md`.

Groups 2 and 3 are kept forever, because the age rule reads the summary's mtime and they have no
summary.

Separately, the repository registry (`~/.local/state/blyte/repos`, `registry.rs`) keeps about 12
paths that no longer exist. `registered()` already skips them, so this is tidiness: the file only
ever grows.

## Decisions

Research-run rules in `check_removable` (`crates/blyte-cli/src/clean.rs`), applied only to
research runs that are not running, after the existing "report copied out" rule:

- **Report still in the worktree** (`.wt/<id>/report.md` exists, `.wt/<id>.report.md` does not):
  removable, with a new reason that says the report will be copied out. The copy happens in the
  removal step, not in `check_removable` (a dry run must not write anything): copy the file to
  `.wt/<id>.report.md`, then remove the worktree. If the copy fails, keep the run and print an
  error naming the file; do not remove it.
- **Ended without a report**: the summary exists and has no report: removable at once, with a
  reason naming the outcome (for example "research run ended without a report (error)").
- **Dispatch stopped without a summary**: no summary and no report: removable when
  `.wt/<id>.start.json` is older than the `--older-than` days, the same threshold the age rule
  uses for the summary. With no start file either, keep it as today ("no report").

Non-research runs are unchanged.

Registry: `blyte clean --all` (not a dry run) rewrites the registry without the paths that are no
longer directories, and prints one line naming them. Write to a temporary file in the same folder
and rename it over the registry. A dispatch that registers a repository between the read and the
rename can lose its line; that is acceptable, because every dispatch registers again.

No fallbacks in either task: an error reading or writing a file is reported, never skipped
silently.

### Task CR1

Make `blyte clean` remove finished research runs that today are kept as "no report".

Files: `crates/blyte-cli/src/clean.rs`, tests under `crates/blyte-cli/tests/` (add a new file,
`test_clean_research_runs.rs`).

1. In `check_removable`, for a research run (`is_research_run`), after the existing rule that
   removes a research run whose report was copied out (`RemovalReason::ResearchReport`) and before
   the merge and age rules, add in this order:
   - If `.wt/<id>/report.md` is a file and `.wt/<id>.report.md` is not: return a new
     `RemovalReason::ResearchReportInWorktree`. Its `Display` says "research report (copied out on
     removal)".
   - If the summary `.wt/<id>.json` exists: return a new `RemovalReason::ResearchEndedWithoutReport(outcome)`,
     where `outcome` is the summary's `outcome` string. If the summary has no `outcome` string or
     cannot be parsed, keep the run with a new `KeptReason` that names the problem (for example
     "unreadable summary"); do not guess an outcome. `Display`: "research run ended without a
     report (<outcome>)".
   - If there is no summary: read the mtime of `.wt/<id>.start.json`. If it is at least
     `older_than_days` old (use `saturating_mul(86400)` as the existing age rule does), return a new
     `RemovalReason::ResearchStoppedWithoutSummary(older_than_days)`; `Display`: "research run
     stopped without a summary, started more than N days ago". If the start file is missing, fall
     through to the existing `KeptReason::NoReport`.
   These rules come after the running check that `check_removable` already does; a running
   research run must still be kept.
2. Find where a `RemovalReason` leads to removing the worktree (the code that calls
   `git worktree remove` for a removable run, around the `match` that lists
   `RemovalReason::ResearchReport` near line 2114). For `ResearchReportInWorktree`, first copy
   `.wt/<id>/report.md` to `.wt/<id>.report.md`; if the copy fails, print
   `error: could not copy <src> to <dst>: <err>` to stderr, keep the run and count it as kept. Only
   then remove the worktree. A dry run copies nothing and prints the reason as usual.
3. Make sure every other `match` on `RemovalReason` handles the three new variants the way it
   handles `ResearchReport` (`git grep -n RemovalReason crates/blyte-cli`).
4. Tests (build a research run's files by hand in a temp git repository, the way the existing clean
   tests do; look at how they create `.wt/<id>.start.json` with `"research": true`):
   - a research run with `report.md` in its worktree and none in `.wt/`: after `clean`, the
     worktree is gone and `.wt/<id>.report.md` has the same contents;
   - the same with `--dry-run`: nothing is copied or removed;
   - a research run with a summary whose outcome is `error` and no report: removed, and the
     printed reason contains "ended without a report (error)";
   - a research run with no summary and a start file whose mtime is set 20 days back (use
     `filetime` if it is already a dev-dependency, otherwise `touch -t` through
     `std::process::Command`): removed; the same with a fresh start file: kept as "no report";
   - a research run with a summary that has no `outcome`: kept, with the new reason;
   - a non-research run with no summary and an old start file: kept as before.

No fallbacks. Do not change the rules for non-research runs.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.

### Task CR2

Make `blyte clean --all` drop registry entries for repositories that no longer exist.

Files: `crates/blyte-cli/src/registry.rs`, and the one call site in `crates/blyte-cli/src/clean.rs`
where `--all` reads `crate::registry::registered()` (around line 2030). Change nothing else in
`clean.rs`.

1. In `registry.rs` add `pub fn prune_missing() -> std::io::Result<Vec<PathBuf>>` (with an inner
   `prune_missing_in(dir: Option<PathBuf>)` for tests, like `registered_from`). It reads the
   registry file, keeps each non-empty line whose path is a directory (dropping duplicates, keeping
   the first), writes the kept lines to a temporary file in the same folder (for example
   `repos.tmp.<pid>`), and renames it over `repos`. It returns the removed paths. If the state
   folder or the registry file does not exist, return an empty list without creating anything.
   Other read or write errors are returned.
2. In `clean.rs`, when `--all` is given and it is not a dry run, call `prune_missing()` before
   reading `registered()`. If it removed paths, print one line:
   `forgot N repositories that no longer exist: <path>, <path>, ...`. If it returns an error,
   print `error: could not tidy the repository registry <path>: <err>` to stderr and go on with
   the clean (the clean itself does not depend on the tidy). With `--dry-run`, call nothing that
   writes; print the line as `would forget N repositories ...` instead, using a read-only check.
3. Tests in `registry.rs`'s test module: a registry with an existing folder, a missing path, a
   duplicate and a blank line ends up with only the existing folder once, and the missing path is
   returned; a missing registry file returns an empty list and creates no file.

No fallbacks.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
