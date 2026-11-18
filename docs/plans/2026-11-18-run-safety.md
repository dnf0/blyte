# Run safety: parallel dispatches keep their worktrees; research reports say what they read

Date: 2026-11-18. Base: `origin/main` at v0.38.2 (76dc404).

## Evidence

**1. Parallel dispatches delete each other's fresh worktrees (major).** Reproduced on 0.38.2 in a
scratch repository: four `blyte dispatch` started together, three lost their worktree within two
seconds (`gate_failed`, "workspace directory ... does not exist"); a second set of four lost three
again. Reported from premise-lookup (4 Nov), spatial-service (5 Nov) and climate-x-data-platform
(11 Nov).

Cause, from the code (`crates/blyte-cli/src/clean.rs`): every dispatch starts `blyte clean --quiet`
in the background. `CleanContext::new` reads every `.wt/*.start.json` once into `start_files`; a file
it cannot read or parse is silently left out. `find_blyte_worktrees` then lists the runs from disk.
A run whose start record appeared, or was half-written, after the snapshot has no entry, so
`check_removable` skips the "is it running" check, and the run's `wip/<id>` branch, which has no
commits yet, is an ancestor of the default branch and counts as merged. The worktree and branch are
removed. `dispatch.rs` writes the start record with a plain `std::fs::write`, so a reader can see
it empty.

**2. Research reports mislead about what was read.** Verdicts on research runs in the last 30 days:
125 accepted, 33 redone, 21 fixed. Two patterns in the notes that a harness can catch:
- citations taken from a working tree on an old or other branch, read as if it were the default
  branch (MASK1, EXP2, R-EBA1, R-PREM1);
- commit ids that appear nowhere in what the run read (GAPPIX, GAPEXP, PROD2).

Left alone: conclusions (not mechanically checkable), PR numbers (line numbers in tool output make
almost any small number "seen"), runs that stop at the limit with a stub (a warning ten turns before
the end already exists).

## Rules for every task

- **No fallbacks.** When something is missing, unreadable or does not match, keep the data and say
  what is wrong by name. Never substitute a default, skip silently or treat "unknown" as "safe to
  remove".
- Only the files the task names. Existing tests pass unmodified unless the task says otherwise.
- Check before committing (all must pass):
  `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- One commit, the exact message given, `git commit --no-gpg-sign`. Do not push.

### Task WL: a running run's worktree is never removed by a parallel clean

**Depends on:** none. **Siblings:** Task RP (adds `provenance.rs` and edits other parts of
`dispatch.rs`) and Task RC (edits `research_check.rs`) are separate and not in this worktree; do
not add them.

**Files:** `crates/blyte-cli/src/clean.rs`, `crates/blyte-cli/src/dispatch.rs`, new
`crates/blyte-cli/tests/test_clean_start_race.rs`.

**What to build**

1. `clean.rs`: `pub fn read_start_record(root: &Path, id: &str) -> Result<Option<serde_json::Value>, String>`
   reading `.wt/<id>.start.json` from disk now. File absent: `Ok(None)`. Read error, or content
   that is not a JSON object (including an empty file): `Err("<path>: <reason>")`.
2. `clean.rs`, `KeptReason`: add `UnreadableStartRecord(String)`, displayed as
   `unreadable start record: <reason>`.
3. `CleanContext::check_removable`: replace `let start_json = self.start_files.get(id);` with a
   fresh `read_start_record(self.root, id)`:
   - `Err(reason)` returns `Err(KeptReason::UnreadableStartRecord(reason))`;
   - `Ok(Some(v))` is stored into `self.start_files` (replacing any snapshot entry) and used for
     the existing running check, unchanged;
   - `Ok(None)` removes any entry for `id` from `self.start_files` and continues as today.
4. `clean_repo_with_options`: in the real (not dry-run) removal branch, immediately before the
   worktree is removed (before the `removed_ok` computation), call `ctx.check_removable(&id,
   older_than_days)` again; if it now returns `Err(reason)`, print `kept .wt/{id} ({reason})`
   unless `quiet`, and `continue` without removing anything or deleting the branch.
5. `dispatch.rs`: `pub fn write_start_record(path: &Path, value: &serde_json::Value) -> std::io::Result<()>`
   that writes pretty JSON plus `\n` to `<path>.tmp` (same directory, the file name with `.tmp`
   appended) and then `std::fs::rename`s it onto `path`. Use it where the start record is written
   today (`let start_path = wt_parent.join(format!("{}.start.json", args.id));`). On `Err`, print
   `error: could not write the start record <path>: <err>` to stderr; do not ignore it silently.

**Tests** (`test_clean_start_race.rs`; copy the `git`, `write_file` and `init_repo` helpers from
`tests/test_clean_running_stale_summary.rs`). Each sets up: `init_repo()`, then
`git worktree add -q -b wip/T1 .wt/T1` (a branch with no commits of its own).

- `a_run_started_after_the_snapshot_is_kept_as_running`: build `CleanContext::new(root)` first;
  then write `.wt/T1.start.json` with `{"id":"T1","pid":<std::process::id()>,"started_at":<now>}`;
  `ctx.check_removable("T1", 14)` is `Err(KeptReason::Running)`.
- `an_empty_start_record_keeps_the_worktree`: `.wt/T1.start.json` is an empty file;
  `blyte_cli::clean::check_removable(root, "T1", 14)` is `Err(KeptReason::UnreadableStartRecord(_))`.
- `a_truncated_start_record_keeps_the_worktree`: content `{"id":"T1","pid":`; same assertion; then
  run `clean_repo_with_options(root, 14, false, None, true, 7, Some(Ok(vec![])))` and assert
  `.wt/T1` still exists and `git rev-parse --verify wip/T1` still succeeds.
- `a_finished_merged_run_is_still_removed`: start record with `"pid": 0`; `check_removable` is
  `Ok(_)` (the fix must not keep everything).
- `write_start_record_leaves_a_whole_file_and_no_temp_file`: after the call the file parses to the
  value written and `<path>.tmp` does not exist.

The first three must fail on the code as it is today.

**Commit message:** `fix(clean): never remove a run whose start record appeared or was half-written during clean`

### Task RP: research prompts and reports state the source state of what was read

**Depends on:** none. **Siblings:** Task WL (edits `clean.rs` and the start-record write in
`dispatch.rs`) and Task RC (edits `research_check.rs`) are separate; do not add them. Do not edit
`RESEARCH_RULES` or `wrap_research`.

**Files:** new `crates/blyte-cli/src/provenance.rs`, `crates/blyte-cli/src/lib.rs` (one
`pub mod provenance;` line), `crates/blyte-cli/src/dispatch.rs`, new
`crates/blyte-cli/tests/test_provenance.rs`.

**What to build**

1. `provenance.rs`:
   ```rust
   pub enum SourceState {
       NotGit,                       // the path is not inside a git work tree
       Git {
           head: String,             // full commit id of HEAD
           branch: Option<String>,   // None when HEAD is detached
           uncommitted: usize,       // lines of `git status --porcelain`
           default_ref: Option<String>,        // e.g. "origin/main"; None when origin/HEAD is not set
           behind_ahead: Option<(usize, usize)>, // HEAD vs default_ref, None when default_ref is None
       },
       Unknown(String),              // a git command failed: the command and its stderr
   }
   pub fn source_state(path: &Path) -> SourceState;
   pub fn render(sources: &[(PathBuf, SourceState)]) -> String;
   ```
   `source_state` runs, in `path`: `git rev-parse --is-inside-work-tree` (failure with "not a git
   repository" in stderr is `NotGit`; any other failure is `Unknown`), `git rev-parse HEAD`,
   `git symbolic-ref --short -q HEAD` (exit 1 with empty output means detached), `git status
   --porcelain`, `git symbolic-ref --short -q refs/remotes/origin/HEAD` (exit 1 means not set),
   and, when set, `git rev-list --left-right --count HEAD...<default_ref>` (first number = ahead,
   second = behind). No fetch, no network. A failing command that is not one of the two named
   "exit 1" cases is `Unknown("<command>: <stderr>")`; never guess a branch or a count.
   `render` returns exactly this shape (one line per source, in the order given):
   ```text
   ## Source state (written by the harness)

   - `/abs/repo`: branch `feat/x` at `0123abc…full id`, 3 behind and 1 ahead of `origin/main` (as last fetched), 2 uncommitted changes
   - `/abs/detached`: detached HEAD at `…`, no `origin/HEAD` to compare with, clean
   - `/abs/cache`: not a git repository
   - `/abs/broken`: unknown (git status --porcelain: fatal: …)

   This is what the files on disk were when the run started. It is not necessarily the default branch.
   Commit ids not listed here or shown in tool output must not be cited.
   ```
   "clean" when `uncommitted == 0`; "level with `origin/main`" when both counts are 0.
2. `dispatch.rs`, research runs only: before the prompt is built, compute the block for `root` and
   then every `--allow-read` root (config ones first, then `args.allow_read`, as they are ordered
   for `--read-root`), skipping duplicates. Pass `format!("{block}\n\n{task_content}")` to
   `wrap_research` in place of `task_content`.
3. `dispatch.rs`, where a non-empty `report.md` is copied to `research_report_path` and
   `report_to`: after each copy, append `"\n\n"` plus the same block to the copy (not to the
   scratch `report.md`). If the append fails, print `error: could not add the source state to
   <path>: <err>` to stderr.

**Tests** (`test_provenance.rs`, temp git repositories, `git -c user.name=test -c
user.email=test@test.com`, commits with `--no-gpg-sign`):

- a repository on branch `main` with one commit and one untracked file: `Git { branch:
  Some("main"), uncommitted: 1, default_ref: None, behind_ahead: None, .. }` and `head` equals
  `git rev-parse HEAD`;
- a clone of it, then one more commit in the origin, `git fetch` in the clone: `default_ref:
  Some("origin/main")`, `behind_ahead: Some((1, 0))`;
- `git checkout --detach` in a repository: `branch: None`;
- a plain temp directory: `NotGit`;
- `render` on one value of each variant contains the heading, each path, `1 behind and 0 ahead of
  \`origin/main\``, `detached HEAD`, `not a git repository`, `unknown (`, and the closing sentence
  about commit ids.

**Commit message:** `feat(research): state each read root's branch, commit and distance from the default branch`

### Task RC: a report may not cite a commit id the run never saw

**Depends on:** none. **Siblings:** Task WL (`clean.rs`, `dispatch.rs`) and Task RP
(`provenance.rs`, `dispatch.rs`) are separate; do not add them. Do not edit `run.rs`.

**Files:** `crates/blyte-agent/src/research_check.rs`, new
`crates/blyte-agent/tests/test_commit_id_check.rs`.

**What to build**

1. `fn is_commit_like(tok: &str) -> bool`: 7 to 40 characters, every one of `0-9a-f`, at least one
   digit and at least one letter.
2. `identifiers(text)`: also yield every token for which `is_commit_like` is true, even when it
   starts with a digit (today those are dropped). Everything it yields today is still yielded.
   Update its doc comment. `run.rs` feeds this into the "seen" set, so commit ids in the task and
   in tool output become seen with no change there.
3. `unseen_names(report, seen)`:
   - the existing name loop must skip tokens that start with a digit, so its results do not change;
   - add a second pass over the same prose lines (outside fenced blocks, the whole line, not only
     backticked spans): for every token `t` (split as `identifiers` splits) with
     `is_commit_like(t)`, it is seen when some entry `s` of `seen` satisfies `s.starts_with(t) ||
     t.starts_with(s)` with `is_commit_like(s)`; otherwise push, once per token,
     `report.md:<line>: commit id `<t>` does not appear in anything this run read (the task, files or command output); cite only commit ids you have seen`.

**Tests** (`test_commit_id_check.rs`; build `seen` with
`identifiers(text).map(str::to_owned).collect()`):

- seen text `commit 76dc4041aa22bb33cc44dd55ee66ff7700112233`; report `Fixed in 76dc404.`: no problems
  (abbreviation of a seen id);
- seen text `76dc404 chore`; report cites the 40-character id starting `76dc404`: no problems;
- empty seen; report `Fixed in 1a2b3c4d.`: one problem naming `1a2b3c4d` and `report.md:1`;
- the same id twice in the report: one problem;
- the id only inside a fenced code block: no problems;
- `20261118` (digits only), `deadbeef` (letters only), `abc123` (6 characters): no problems;
- `identifiers("x 1a2b3c4 foo_bar 12")` yields `1a2b3c4` and `foo_bar` and not `12`;
- the existing `tests/test_name_check.rs` passes unmodified.

**Commit message:** `feat(research): flag commit ids in a report that the run never saw`
