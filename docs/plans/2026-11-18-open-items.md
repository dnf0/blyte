# Open items after v0.42.0: toolchain pin, path check, saved review flags

Date: 2026-11-18. Follows `docs/plans/2026-11-18-report-and-run-checks.md`.

## What was checked first

- **Idle timeout (no task).** 13 runs recorded stall timing on 2026-11-18 (153 turns, all streaming, one model).
  Time to first data: median 14 s, 95th percentile 85 s, longest 246 s. Longest silence in mid-stream, after the
  first data: up to 108 s. No call timed out and none was retried. Four healthy turns were silent for more than
  120 s, so any default below about 250 s would have ended a healthy call. The 300 s default stays. Look again
  when a run records a timed-out attempt: that is the first evidence of what a stall looks like.
- **Fallback check counts cannot be made after the fact.** `blyte clean` removes a run's worktree and branch once
  it is merged, and `blyte review <id>` then fails ("worktree does not exist"). Of 130 past runs with a verdict,
  none could be re-checked. The flags must be saved when the review runs: Task FV.
- **Duplicate release.** A release run checks out the commit that started it. When a second merge lands before
  the first release commit, the second run bumps from an old tree, lands an empty release commit and fails at
  the tag. Fixed directly in the workflow (not a task: no local check covers it): check out `main`, and stop
  before the release pull request if the tag exists.
- **CI toolchain.** All three workflows install `dtolnay/rust-toolchain@stable`, so a new stable Rust can fail
  `main` with no change of ours (clippy 1.100 did, 2026-11-18). Task TC.
- **Path check.** Two findings from running it on 45 saved reports: it does not exempt "Add ... `path`" for a
  proposed file, and of two paths separated by one space only the first is checked. Task PW.

The three tasks touch different files and run in parallel.

### Task TC: pin the Rust toolchain

**Goal.** CI, release builds and local checks all use one named Rust version, so a new stable release cannot
break `main`. The version is 1.100.0.

**Depends on:** none.

**Siblings (not in this worktree; do not touch their files):** PW changes
`crates/blyte-agent/src/research_check.rs`; FV changes `crates/blyte-cli/src/dispatch.rs` and
`crates/blyte-cli/src/fallback_check.rs`.

**Files.**
- Create `rust-toolchain.toml` at the repository root.
- Change `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.github/workflows/release-on-merge.yml`.
- Create `crates/blyte-cli/tests/test_toolchain_pin.rs`.

**What to build.**
1. `rust-toolchain.toml`:
   ```toml
   # The one Rust version for CI, release builds and local checks. To move to a newer Rust, change
   # it here and in every `toolchain:` line of .github/workflows (a test keeps them equal).
   [toolchain]
   channel = "1.100.0"
   components = ["rustfmt", "clippy"]
   ```
2. In the three workflow files, replace every `uses: dtolnay/rust-toolchain@stable` with
   `uses: dtolnay/rust-toolchain@master` and add `toolchain: "1.100.0"` to that step's `with:` block. Keep the
   step's existing `with:` entries (`components`, `targets`); where the step has no `with:` block, add one.
   There are four such steps: two in `ci.yml`, one in `release.yml`, one in `release-on-merge.yml`.
3. Change nothing else in the workflows.

**Tests** (`crates/blyte-cli/tests/test_toolchain_pin.rs`; find the repository root from
`env!("CARGO_MANIFEST_DIR")`, two levels up). No fallbacks: a missing file or a missing `channel` is a test
failure with a message naming it, never a skip.
- `the_toolchain_file_names_an_exact_version`: `rust-toolchain.toml` has `channel = "X.Y.Z"` with three numeric
  parts (not `stable`, `beta`, `nightly`).
- `no_workflow_installs_a_floating_toolchain`: no file in `.github/workflows` contains
  `dtolnay/rust-toolchain@stable`, `@beta` or `@nightly`.
- `every_workflow_toolchain_matches_the_file`: for every line containing `dtolnay/rust-toolchain@` in every
  workflow file, a `toolchain:` line follows within the next five lines, and its value (quotes stripped) equals
  the channel in `rust-toolchain.toml`. The test also asserts it found at least four such steps.

Run: `cargo test -p blyte --test test_toolchain_pin`. The tests fail before the change (no
`rust-toolchain.toml`).

**Constraints.** Existing tests pass unmodified. This is not a long-running job, so nothing here needs to be
resumable.

**Commit message:** `ci: pin the Rust toolchain to 1.100.0`

### Task PW: path check sees adjacent paths and accepts "add" for a new file

**Goal.** Close two gaps in `unseen_paths` (`crates/blyte-agent/src/research_check.rs`), found by running it on
real reports.

**Depends on:** none.

**Siblings (not in this worktree; do not touch their files):** TC adds `rust-toolchain.toml` and changes the
workflows; FV changes `crates/blyte-cli/src/dispatch.rs` and `crates/blyte-cli/src/fallback_check.rs`.

**Files.** `crates/blyte-agent/src/research_check.rs`, `crates/blyte-agent/tests/test_unseen_paths.rs`.

**What to build.**
1. **Adjacent paths.** `prose_path_re` consumes the delimiter after a path, so in `a/x.rs b/y.rs` the single
   space is used up by the first match and the second path is never found. Make `paths(text)` return every
   path, including one that starts right after the delimiter that ended the previous one. The `regex` crate has
   no look-ahead: either search again from the end of the captured path (not the end of the whole match), or
   drop the trailing delimiter from the pattern and check the character after the capture in code. The set of
   delimiters before and after a path stays exactly as it is now. `paths` keeps its order (as written) and
   returns each occurrence once.
2. **"Add" means new.** Add the whole words `add`, `adds`, `added`, `adding` (case-insensitive) to
   `missing_or_new_re`, next to `new`, `create`, `created`. Whole words only: `address` and `padded` do not
   match.

**Tests** (add to `crates/blyte-agent/tests/test_unseen_paths.rs`, using its `problems(report, seen)` helper;
the paths below do not exist in the test workspace and are not in `seen`).
- `two_paths_one_space_apart_are_both_flagged`: report `See src/nope_a.rs src/nope_b.rs for details.` gives two
  problems, one naming `src/nope_a.rs` and one naming `src/nope_b.rs`.
- `two_backticked_paths_one_space_apart_are_both_flagged`: the same with each path in backticks.
- `three_paths_in_a_row_are_all_flagged`: `src/nope_a.rs src/nope_b.rs src/nope_c.rs` gives three problems.
- `a_file_to_add_is_clean`: `- \`src/nope_new.rs\`: Add tests for the parser.` gives no problems; the same for
  `Adding`, `adds` and `added`.
- `address_is_not_add`: `We address this in \`src/nope_a.rs\`.` gives one problem.
- A unit test for `paths`: `paths("a/x.rs b/y.rs")` equals `["a/x.rs", "b/y.rs"]`, and
  `paths("see a/x.rs, then b/y.rs.")` equals the same.

Run: `cargo test -p blyte-agent --test test_unseen_paths`. The first three and `a_file_to_add_is_clean` fail
before the change.

**Constraints.** Existing tests pass unmodified. No fallbacks: no new default or silent skip. Not a
long-running job, so nothing here needs to be resumable.

**Commit message:** `fix(research): the path check sees adjacent paths and accepts "add" for a new file`

### Task FV: save a review's check flags, so they can be counted after clean

**Goal.** `blyte review <id>` prints its flags under `== checks` and keeps nothing. Once the run is merged,
`blyte clean` removes the worktree and the review cannot be repeated, so flags cannot be counted against
verdicts. Save them when the review runs.

**Depends on:** none.

**Siblings (not in this worktree; do not touch their files):** TC adds `rust-toolchain.toml`, changes the
workflows and adds `crates/blyte-cli/tests/test_toolchain_pin.rs`; PW changes
`crates/blyte-agent/src/research_check.rs`.

**Files.** `crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/src/fallback_check.rs`, new
`crates/blyte-cli/tests/test_review_checks_file.rs`.

**What to build.**
1. In `run_review` (`dispatch.rs`), on the path that prints `== checks` (not the `--verdict` path, which returns
   earlier), write `.wt/<id>.checks.json` in the main checkout's `.wt` directory, next to `<id>.json`, before
   printing `== checks`:
   ```json
   {
     "ts": 1795012354,
     "flags": ["<each flag exactly as printed, without the leading '! '>"],
     "fallbacks": [{"file": "src/a.rs", "line": 12, "pattern": "unwrap_or"}]
   }
   ```
   - `flags` is `checks.flags`, in order; an empty array when nothing is flagged.
   - `fallbacks` is every `Finding` from `fallback_check::added_fallbacks`, all of them and in full (the printed
     flag lists only three and is cut short). For a research run, or when the diff could not be read, it is an
     empty array; the "fallback check not run" flag is then in `flags`.
   - `ts` is seconds since the Unix epoch.
   - Each review overwrites the file.
   - No fallbacks: if the file cannot be written, print `error writing <path>: <reason>` to stderr and return 2.
2. Derive `serde::Serialize` for `fallback_check::Finding` (fields as they are).
3. A new dispatch of the same id archives the earlier run's files: add `"checks.json"` to that list of
   extensions (the loop over `["json", "report.json", "start.json", "verdict.json", "log"]` in `dispatch.rs`).
4. Recording a verdict (`--verdict`) does not write, change or remove `checks.json`.

**Tests** (`crates/blyte-cli/tests/test_review_checks_file.rs`; build the run fixture the way
`crates/blyte-cli/tests/test_review_base.rs` does).
- `review_writes_the_checks_file`: a run whose commit adds a line `let v = x.unwrap_or(0);` to a file the task
  names. After `blyte review <id>`, `.wt/<id>.checks.json` exists; `flags` has one entry starting
  `possible fallbacks added (1)`; `fallbacks` has one entry with that file, a line number and a non-empty
  `pattern`; `ts` is a number greater than 0.
- `all_fallbacks_are_saved_not_three`: a commit adding five such lines: `fallbacks` has five entries while the
  printed flag says `and 2 more`.
- `a_clean_review_writes_empty_arrays`: a run with nothing to flag: `flags` and `fallbacks` are `[]`, and stdout
  has `nothing flagged`.
- `recording_a_verdict_leaves_the_checks_file`: after a review and then
  `blyte review <id> --verdict accepted`, the file's bytes are unchanged.
- `a_second_review_overwrites`: review, add a flagged commit to the run's branch, review again: the file shows
  the new flags.

Run: `cargo test -p blyte --test test_review_checks_file`. All fail before the change (no file is written).

**Constraints.** Existing tests pass unmodified. The printed output of `blyte review` does not change. Not a
long-running job, so nothing here needs to be resumable.

**Commit message:** `feat(review): save each review's check flags to .wt/<id>.checks.json`

## After the tasks

- Merge each by pull request.
- Two weeks after FV is installed (2026-12-02): join `.wt/*.checks.json` with `.wt/*.verdict.json` and count,
  per fallback pattern, how many flagged runs were `accepted` unchanged. Narrow the fallback check from that.
- Path check: read `flags` in the same files for research runs flagged on an unseen path, and check each by
  hand against the report.
