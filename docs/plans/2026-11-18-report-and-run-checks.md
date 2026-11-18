# Report and run checks: plan (2026-11-18)

Four gaps left after v0.40.0, each found in real runs:

| Item | Gap | Tasks |
|---|---|---|
| 3 | A research run that hits its limit leaves an early stub as its report, and the copy looks like a finished report | ST1, ST2 |
| 4 | A report can name a file path, pull request or issue number that the run never saw and that does not exist | PA, PN |
| 5 | An implementation run can add a fallback (a default standing in for "unknown") and nothing in `blyte review` points at it | FB |
| 6 | A run's worktree exists on disk before its start record does, so for a moment nothing says who owns it | SR |

The facts below were checked against `origin/main` at `911cdd1` (v0.40.0), using four research runs
(R-STUB, R-PATHS, R-FALLBACK, R-START) and then by reading the cited lines.

## Order

Tasks that change the same file run one after another, so there are two chains, and the two chains run
in parallel:

- `blyte-cli` chain: **SR**, then **ST2** (`--base wip/SR`), then **FB** (`--base wip/ST2`). All three change `crates/blyte-cli/src/dispatch.rs`.
- `blyte-agent` chain: **ST1**, then **PA** (`--base wip/ST1`), then **PN** (`--base wip/PA`). All three change `crates/blyte-agent/src/run.rs` or `research_check.rs`.

Dispatch SR and ST1 together first. Each later task is dispatched when the one before it has been reviewed.

## Rules for every task

- **No fallbacks.** When something is missing or unreadable, fail or flag it with a message that names it.
  Never substitute a default, a guess, an empty result or a silent skip. If a fallback seems unavoidable, stop
  and say so in the final message instead of writing one.
- Existing tests pass unmodified, unless the task says otherwise.
- Check with the repository's gate. CI uses Rust 1.100: if `cargo +1.100.0` is installed, run
  `cargo +1.100.0 clippy --all-targets -- -D warnings` before committing.
- One commit, with the exact message given.

---

### Task SR: the start record is written before the worktree exists

**Goal.** `blyte dispatch` writes `.wt/<id>.start.json` before it creates `.wt/<id>` (the worktree, or the
scratch directory of a research run), so a directory under `.wt/` never exists without a start record naming
a live process.

**Why.** Today `run_dispatch` starts the background clean (`dispatch.rs:517`), creates the scratch directory
or worktree (`dispatch.rs:520-640`), and writes the start record only after the child is spawned
(`dispatch.rs:962`). Between those points another dispatch's clean sees a directory with no start record. It
is kept today only because clean treats such a directory as a recently used session worktree.

**Depends on:** none.

**Siblings.** ST1 runs at the same time in `crates/blyte-agent`; it is not in this worktree. ST2 and FB
change other parts of `dispatch.rs` later; do not add anything of theirs.

**Files.**
- `crates/blyte-cli/src/dispatch.rs` (`run_dispatch` only)
- `crates/blyte-cli/tests/test_dispatch_start_first.rs` (new)

**What to build.**

1. An initial start record, written with the existing `write_start_record`, at this point in `run_dispatch`:
   after the checks on an earlier run with the same id (already running, replaced and archived) and before
   the first of: the call to `crate::clean::spawn_background_clean`, `create_dir_all(&worktree_dir)`, and
   `git worktree add`. It applies to research and non-research runs alike. Create `.wt/` first if it is
   missing. Its fields:
   - `id`, `started_at` (sampled here, once), `tags` (with `research` added for a research run, as today),
     `plan`, `plan_task`, `base`;
   - `pid`: `std::process::id()`, the dispatch process itself, which stays alive until the run ends;
   - `starting`: `true`;
   - `session` and `research`, under the same conditions as today.
   If this write fails, print `error: could not write the start record <path>: <err>` and return 2. Do not
   create the directory.
2. A guard that removes the record when dispatch gives up. Add
   `struct StartRecordGuard { path: PathBuf, armed: bool }` with a `Drop` that removes `path` when `armed`.
   Create it straight after the initial write. Every `return 2` between the initial write and a successful
   `cmd.spawn()` then removes the record without further code. Set `armed = false` once `cmd.spawn()` has
   succeeded.
3. The final record, written where the record is written today (after `cmd.spawn()`): exactly today's
   fields and values, with `pid` the child's id, `started_at` the value sampled in step 1 (not sampled
   again), and no `starting` key. Keep today's error message if this second write fails.

Do not change `clean.rs`, `wait.rs`, `dashboard.rs` or `plan_cmd.rs`: they read `pid`, and a live dispatch
pid already means "running" to them.

**Tests** (`crates/blyte-cli/tests/test_dispatch_start_first.rs`). Use the `git` helper and the command
setup of `crates/blyte-cli/tests/test_dispatch_plan_fields.rs` (`--gate true --sandbox off`,
`BLYTE_STATE_DIR`, `BLYTE_NO_UPDATE_CHECK=1`, model keys removed: the child fails for want of a key, which
is fine here).

1. `the_start_record_exists_before_the_worktree` (fails on current code). In a new repository with one
   commit and a `task.md`, install an executable `.git/hooks/post-checkout`:

   ```sh
   #!/bin/sh
   top="$(git rev-parse --git-common-dir)/.."
   if [ -f "$top/.wt/S1.start.json" ]; then cp "$top/.wt/S1.start.json" "$top/seen-start.json"; fi
   ```

   (`git worktree add` runs this hook inside the new worktree.) Run `blyte dispatch S1 task.md ...`. Assert:
   `seen-start.json` exists; parsed, `starting` is `true`, `id` is `"S1"`, and `pid` is an integer above 0.
   Then read `.wt/S1.start.json` as left at the end: it has no `starting` key, its `pid` differs from the
   one in `seen-start.json`, and its `started_at` equals the one in `seen-start.json`.
2. `a_failed_start_leaves_no_start_record`. Run `blyte dispatch S2 task.md --base no-such-ref ...`. Assert
   the exit code is 2 and neither `.wt/S2.start.json` nor `.wt/S2` exists.
3. `a_research_run_ends_with_a_complete_start_record`. Run `blyte dispatch S3 task.md --research` (with the
   same environment). Assert `.wt/S3.start.json` has `research: true`, no `starting` key, and the keys
   `gate`, `max_turns` and `max_seconds`.

Run: `cargo test -p blyte --test test_dispatch_start_first`, then the full gate.

**Commit message:** `fix(dispatch): write the start record before the worktree exists`

---

### Task ST1: other tools close after the wrap-up warning until report.md changes

**Goal.** In a research run, once the wrap-up warning has been sent, every tool except `write` and `edit` is
closed until `report.md` has changed. A run can no longer spend its last turns reading while an early stub
sits untouched.

**Why.** The warning ten turns (or 10% of the time) before the end (`run.rs:969-989`) is only a message. Runs
have ignored it, carried on reading until `BudgetExhausted`, and left the skeleton written at turn 8 as the
report.

**Depends on:** none.

**Siblings.** SR runs at the same time in `crates/blyte-cli`; it is not in this worktree. PA and PN change
`research_check.rs` and one other function of `run.rs` later; do not add anything of theirs.

**Files.**
- `crates/blyte-agent/src/run.rs` (`run_loop` only)
- `crates/blyte-agent/tests/test_research_wrap_up.rs` (new)

**What to build.**

1. When the wrap-up message is pushed (either branch: turns or time) and `self.research` is true, record the
   bytes of `<root>/report.md` at that moment (`None` when the file is missing), read through `self.fs`.
2. For each tool call after that, in a research run: the tools are *closed* while `report.md` is missing, is
   empty, or has the same bytes as recorded. Check the file before each tool call. Once it has been seen
   non-empty and different, the tools are open for the rest of the run (do not close them again).
3. While closed, a call to any tool other than `write` or `edit` is not executed. It gets, by the same
   mechanism as `read_closed` (`run.rs:1314-1320`), a `tools::ToolOutcome` with `is_error: true` and exactly:

   ```text
   [blyte] almost out of budget: write report.md now (write or edit). Other tools are closed until report.md has changed.
   ```

   Treat such a call like a `read_closed` call in the bookkeeping that follows: its content is not added to
   the seen names and it is not counted as a repeated read. `write` and `edit` run as normal, to any path.
4. Non-research runs are unchanged. The existing `read_closed` rule is unchanged and takes precedence when
   both apply.

**Tests** (`crates/blyte-agent/tests/test_research_wrap_up.rs`). Copy the `Runner` setup of
`crates/blyte-agent/tests/test_research_reminder.rs` (`LocalShell`, `LocalFs`, `Conversational`,
`claim_check: false`, `max_gate_attempts: 1`, `max_duration: Some(Duration::from_secs(300))`) with
`max_turns: 25`, so the warning is sent at the start of turn 16. Write a scripted provider that on turn `n`
calls `bash` with `touch m<n>`, except on the turns given to it as "write turns", where it calls `write` with
`{"path": "report.md", "content": "# Findings <n>\n"}`. A marker file `m<n>` in the root then shows that the
bash call of turn `n` ran.

1. `tools_close_after_the_warning_until_the_report_changes` (fails on current code): write turns `[]`.
   Assert `m15` exists and none of `m16` ... `m25` exists. Assert that a request seen by the provider after
   turn 16 contains the message text above.
2. `an_early_stub_does_not_open_the_tools` (fails on current code): write turns `[5]`. Assert `m15` exists
   and `m16`, `m20` and `m25` do not.
3. `writing_the_report_opens_the_tools_again`: write turns `[18]`. Assert `m16` and `m17` do not exist and
   `m19` and `m25` do.
4. `rewriting_the_same_bytes_does_not_open_the_tools` (fails on current code): the provider writes
   `"# Stub\n"` on turns 5 and 18 (give the provider a fixed content for this test). Assert `m19` does not
   exist.
5. `an_implementation_run_is_not_affected`: `research: false`, write turns `[]`. Assert `m20` exists.

Give every test a generous timeout if it uses one (300 s, not 30 s): CI runners are slow.

Run: `cargo test -p blyte-agent --test test_research_wrap_up`, then the full gate.

**Commit message:** `fix(research): close other tools after the wrap-up warning until report.md changes`

---

### Task ST2: the copy of an unverified report says so

**Goal.** The copy of a research report that did not end `verified` starts with a line saying so. A stub or
half-finished report can no longer pass for a finished one.

**Why.** `run_dispatch` copies any non-empty `report.md` to `.wt/<id>.report.md` and to `--report-to`
whatever the outcome (`dispatch.rs:986-1008`). Readers open that file, not the summary JSON.

**Depends on:** SR (same function of `dispatch.rs`). Dispatch with `--base wip/SR`.

**Siblings.** ST1 makes the stub rarer in `crates/blyte-agent`; it is not in this worktree and this task does
not need it. FB changes `run_review` later.

**Files.**
- `crates/blyte-cli/src/provenance.rs`
- `crates/blyte-cli/src/dispatch.rs` (`run_dispatch` only)
- `crates/blyte-cli/tests/test_unverified_report.rs` (new)

**What to build.**

1. In `provenance.rs`:

   ```rust
   /// The first lines of a research report whose run did not end `verified`.
   pub fn unverified_banner(outcome: &str, turns: Option<u64>) -> String

   /// Put `unverified_banner` at the top of the report at `path`, unless `outcome` is "verified".
   /// The rest of the file is kept byte for byte. A file that cannot be read or written is an error.
   pub fn mark_if_unverified(path: &Path, outcome: &str, turns: Option<u64>) -> std::io::Result<()>
   ```

   `unverified_banner("budget_exhausted", Some(300))` returns exactly:

   ```text
   > **Not verified.** This research run ended as `budget_exhausted` after 300 turns. The report below did not pass blyte's checks and may be an unfinished draft: check it before relying on it.

   ```

   (one line, then one blank line). With `turns` `None`, the words ` after N turns` are left out and nothing
   is put in their place. `mark_if_unverified` with outcome `"verified"` returns `Ok(())` and does not touch
   the file. A file that already starts with the banner for the same outcome and turns is left as it is.
2. In `run_dispatch`, for a research run, once the run's outcome string (the `outcome` printed in the summary
   JSON) and its turn count (from the run report, `None` when the report could not be read) are known: call
   `mark_if_unverified` on `.wt/<id>.report.md` and on the `--report-to` copy, for each of the two that
   exists. The source-state block stays at the end, as today. If the call fails, print
   `error: could not mark <path> as not verified: <err>` to stderr; do not change the exit code.

**Tests** (`crates/blyte-cli/tests/test_unverified_report.rs`), on the two functions:

1. `the_banner_names_the_outcome_and_the_turns`: the exact string above.
2. `the_banner_without_a_turn_count`: `unverified_banner("error", None)` equals
   ``"> **Not verified.** This research run ended as `error`. The report below did not pass blyte's checks and may be an unfinished draft: check it before relying on it.\n\n"``.
3. `an_unverified_report_gets_the_banner_first`: a temp file `"# Stub\n\nbody\n"`;
   `mark_if_unverified(p, "budget_exhausted", Some(300))`; the file equals the banner followed by the
   original bytes.
4. `a_verified_report_is_untouched`: same file, outcome `"verified"`; bytes unchanged.
5. `marking_twice_adds_one_banner`: call twice; the file equals the result of one call.
6. `a_missing_report_is_an_error`: a path that does not exist returns `Err`, and no file is created.

All six fail to compile on current code, which is the failing state. In the final message, quote the lines
of `run_dispatch` where the two calls were added: there is no test of the wiring, so the reviewer reads it.

Run: `cargo test -p blyte --test test_unverified_report`, then the full gate.

**Commit message:** `feat(research): mark the copy of a report that was not verified`

---

### Task PA: a report may not name a file that does not exist and was never seen

**Goal.** A research report may not name, in prose, a source file that does not exist and that the run never
saw.

**Why.** A path is checked today only when it carries a line number or leads into a quoted block. A path in
plain prose (`the handler in src/api/routes.py`) is not checked anywhere: `check_prose` only handles
citations with lines, and `unseen_names` splits a backticked path into words. Reports have named files that
do not exist.

**Depends on:** ST1 (same file, `run.rs`). Dispatch with `--base wip/ST1`.

**Siblings.** PN adds a check for pull request numbers afterwards; do not add it here.

**Files.**
- `crates/blyte-agent/src/research_check.rs`
- `crates/blyte-agent/src/run.rs` (`check_report_quotes` only)
- `crates/blyte-agent/tests/test_unseen_paths.rs` (new)

**What to build.**

1. A regex for a path in prose, used by everything below. It differs from `path_re()` in requiring at least
   one `/`, allowing a leading `/`, and allowing `.` or `;` after the path:

   ```text
   (?:^|[\s`(\[])(/?(?:[\w.~-]+/)+(?:[\w.-]*\.[A-Za-z][A-Za-z0-9_]*|BARE_NAMES))(?:$|[\s`):,\].;])
   ```

   (`BARE_NAMES` is the existing constant.) Because the path must follow whitespace, a backtick, `(` or `[`,
   a URL (`https://host/a/b.py`), a glob (`src/**/*.rs`) and a placeholder (`runs/<id>/x.json`) do not match.
2. `pub fn paths(text: &str) -> impl Iterator<Item = &str>`: every such path in `text`, as written.
3. `identifiers(text)` also yields everything `paths(text)` yields. This puts every path in the task and in
   tool output into the run's seen set with no change to the three places in `run.rs` that fill it. A string
   with `/` in it cannot affect the name check. If an existing test asserts the exact output of `identifiers`
   on text that contains such a path and fails, stop and report it; do not edit that test.
4. `pub fn unseen_paths(report: &str, src: &Sources<'_>, seen: &HashSet<String>) -> Vec<String>`. Walk the
   report's lines, skipping fenced blocks exactly as `unseen_names` does. On each prose line, for each path
   `p` from `paths(line)` (with a leading `./` removed), report a problem unless one of these holds:
   - its extension (lower-cased) is not in `CODE_EXTENSIONS` and its file name is not one of `BARE_NAMES`;
   - it starts with `~` or `..`;
   - `citations(line)` already yields the same path (it is checked with its line numbers);
   - the line says the file is not there or is to be made: it contains, as a whole word and in any case,
     one of `new`, `create`, `creates`, `created`, `propose`, `proposed`, `missing`, `deleted`, `removed`,
     `renamed`, `absent`, or contains `does not exist`, `do not exist` or `no such file`;
   - the next non-blank line opens a fenced block tagged `proposed`, `diff` or `patch`;
   - the path resolves: `Resolver::resolve` (the existing resolver, one per call of `unseen_paths`) returns
     `Files` or `Skip`;
   - the run saw it: `seen` holds a string `s` containing `/` with `s == p`, or `s` ending in `/` + `p`, or
     `p` ending in `/` + `s`.

   Report each path once, at its first line:

   ```text
   report.md:<line>: `<p>`: no such file, and the path does not appear in anything this run read (the task, files or command output); name only files you have seen, or say on the same line that the file is new or missing
   ```
5. In `run.rs`, `check_report_quotes`: bind the `Sources` passed to `research_check::check` to a variable,
   and after the `unseen_names` call add
   `problems.extend(research_check::unseen_paths(&report, &sources, seen_names));`.

**Tests** (`crates/blyte-agent/tests/test_unseen_paths.rs`). Build `Sources` over a temp directory the way
`crates/blyte-agent/tests/test_research_check.rs` does, containing `src/api/routes.py` and `Makefile`, with a
helper `problems(report: &str, seen: &[&str]) -> Vec<String>`. One test per row; "flagged" means exactly one
problem, which contains the path in backticks and `no such file`; "clean" means no problems.

| Test | Report text | Seen | Expect |
|---|---|---|---|
| `an_invented_path_in_prose_is_flagged` | `The handler is in src/api/handlers.py.` | none | flagged |
| `an_invented_backticked_path_is_flagged` | ``See `src/api/handlers.py` for it.`` | none | flagged |
| `an_invented_absolute_path_is_flagged` | `Read /nonexistent-root/src/a.rs first.` | none | flagged |
| `an_existing_path_is_clean` | `The handler is in src/api/routes.py.` | none | clean |
| `a_path_seen_in_tool_output_is_clean` | `The workflow calls scripts/deploy.sh, which fails.` | `scripts/deploy.sh` | clean |
| `a_seen_longer_path_covers_its_suffix` | ``It lives in `api/handlers.py`.`` | `/repo/src/api/handlers.py` | clean |
| `a_path_said_to_be_new_is_clean` | ``Put the check in a new `src/api/checks.py`.`` | none | clean |
| `a_path_said_not_to_exist_is_clean` | ``The config names `scripts/deploy.sh`, which does not exist.`` | none | clean |
| `a_lead_in_to_a_proposed_block_is_clean` | ``Draft `jobs/drought.yaml`:`` then a ```` ```proposed ```` block | none | clean |
| `a_url_is_clean` | `See https://example.com/a/b.py for the source.` | none | clean |
| `a_glob_and_a_placeholder_are_clean` | ``Match `src/**/*.rs` and `runs/<id>/x.json`.`` | none | clean |
| `a_branch_and_a_mime_type_are_clean` | ``Based on `origin/main`, served as `application/json`.`` | none | clean |
| `a_non_code_extension_is_clean` | `The notes are in docs/notes/missing.md.` | none | clean |
| `a_path_inside_a_fenced_block_is_clean` | a ```` ```text ```` block containing `src/api/handlers.py` | none | clean |
| `a_path_is_reported_once` | the first row's sentence on two lines | none | one problem, naming line 1 |
| `a_cited_path_is_left_to_the_line_check` | `See src/api/handlers.py:12 for it.` | none | clean from `unseen_paths` |

Two more:

- `identifiers_yield_paths`: `identifiers("ran scripts/deploy.sh and foo_bar")` contains both
  `scripts/deploy.sh` and `foo_bar`.
- `paths_finds_only_slash_paths`: `paths("see routes.py and src/api/routes.py.")` yields exactly
  `["src/api/routes.py"]`.

Rows 1, 2, 3 and 15 are the ones that would pass today for the wrong reason if the function returned nothing,
so the file fails to compile on current code and those rows fail on an empty implementation.

Run: `cargo test -p blyte-agent --test test_unseen_paths`, then the full gate.

**Commit message:** `feat(research): reports may not name a file that does not exist and was never seen`

---

### Task PN: a report may not cite a pull request or issue number that was never seen

**Goal.** A research report may not cite a pull request or issue number that appeared nowhere in what the
run read.

**Why.** Reports have cited pull request numbers that the run never looked up. The check for commit ids
(v0.40.0) covers hashes only.

**Depends on:** PA (same file, and it reuses the way PA feeds the seen set). Dispatch with `--base wip/PA`.

**Siblings.** None running at the same time in this crate.

**Files.**
- `crates/blyte-agent/src/research_check.rs`
- `crates/blyte-agent/tests/test_unseen_change_refs.rs` (new)

**What to build.**

1. `pub fn change_refs(text: &str) -> impl Iterator<Item = &str>`: the digits (only the digits, as a slice
   of `text`) of every reference of these forms:
   - `#N` with two to six digits, at the start of the text or after whitespace, `(` or `[`, and not followed
     by a letter, digit or `_` (so `#1`, `#fff` and `#000000a` are not references);
   - `PR N`, `PR #N`, `pull request N`, `pull request #N`, `issue N`, `issue #N`, in any case, as whole
     words, with one to six digits;
   - `/pull/N` and `/issues/N`;
   - `"number": N` (any spacing after the colon);
   - a line that starts with one to six digits followed by a tab (a row of `gh pr list` output).
2. `identifiers(text)` also yields everything `change_refs(text)` yields. A string of digits only is yielded
   by nothing else in `identifiers`, so in the seen set it can only have come from a reference.
3. `pub fn unseen_change_refs(report: &str, seen: &HashSet<String>) -> Vec<String>`: over the report's prose
   lines (fenced blocks skipped as in `unseen_names`) with inline code spans removed, each number from the
   first two forms above and from `/pull/N` and `/issues/N` that is not in `seen`, reported once at its first
   line:

   ```text
   report.md:<line>: #<N> does not appear in anything this run read (the task, files or command output); cite only pull requests and issues you have seen
   ```
4. `unseen_names` ends by also extending its problems with `unseen_change_refs(report, seen)`, next to the
   existing `unseen_commit_ids` call. `run.rs` does not change.

**Tests** (`crates/blyte-agent/tests/test_unseen_change_refs.rs`). `seen` is built with
`identifiers(tool_output)` in each test, as `run.rs` builds it.

| Test | Report line | Tool output seen | Expect |
|---|---|---|---|
| `an_invented_pr_number_is_flagged` | `This was fixed in #512.` | none | one problem containing `#512` |
| `an_invented_pr_word_form_is_flagged` | `PR 7 changed the parser.` | none | one problem containing `#7` |
| `a_number_seen_in_a_commit_subject_is_clean` | `This was fixed in #512.` | `fix(clean): keep runs (#512)` | clean |
| `a_number_seen_in_a_url_is_clean` | `See pull request 512.` | `https://github.com/o/r/pull/512` | clean |
| `a_number_seen_in_gh_list_output_is_clean` | `PR #341 is the release.` | `341\tchore(release): v0.40.0\tmain\tMERGED` | clean |
| `a_number_seen_in_gh_json_is_clean` | `Issue 88 tracks it.` | `{"number": 88, "title": "x"}` | clean |
| `a_line_number_is_not_a_seen_reference` | `This was fixed in #120.` | `120: let x = 1;` and `src/a.rs:120` | one problem |
| `a_rank_is_not_a_reference` | `The #1 cause is the cache.` | none | clean |
| `a_number_in_code_is_clean` | ``Set the colour to `#102030`.`` | none | clean |
| `a_number_in_a_fenced_block_is_clean` | a ```` ```text ```` block containing `#512` | none | clean |
| `a_number_is_reported_once` | `#512` on two lines | none | one problem, naming line 1 |
| `unseen_names_includes_the_references` | `This was fixed in #512.` through `unseen_names` | none | one problem containing `#512` |

Run: `cargo test -p blyte-agent --test test_unseen_change_refs`, then the full gate.

**Commit message:** `feat(research): reports may not cite a pull request or issue number that was never seen`

---

### Task FB: review flags fallbacks a run added

**Goal.** `blyte review` flags, under `== checks`, lines an implementation run added that look like a
fallback, so the reviewer reads each one.

**Why.** Tasks say "no fallbacks", and runs have still added a default standing in for "unknown". The gate
cannot see that, and the reviewer usually reads the summary, not the whole diff. This is a pointer for the
reviewer, not a verdict: it never changes the outcome or the exit code.

**Depends on:** ST2 (same file, `dispatch.rs`). Dispatch with `--base wip/ST2`.

**Siblings.** None running at the same time in this crate.

**Files.**
- `crates/blyte-cli/src/fallback_check.rs` (new), `crates/blyte-cli/src/lib.rs` (`pub mod fallback_check;`)
- `crates/blyte-cli/src/dispatch.rs` (`run_review` only)
- `crates/blyte-cli/tests/test_fallback_check.rs` (new)

**What to build.**

1. In `fallback_check.rs`:

   ```rust
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct Finding { pub file: String, pub line: usize, pub pattern: &'static str }

   /// Added lines of a unified diff (`git diff` output) that match a fallback pattern.
   pub fn added_fallbacks(diff: &str) -> Vec<Finding>

   /// One `== checks` flag for the findings, or `None` when there are none.
   pub fn flag(findings: &[Finding]) -> Option<String>
   ```

2. Reading the diff: `+++ b/<path>` sets the current file (`+++ /dev/null` means a deleted file: ignore
   until the next file); `@@ -a,b +c,d @@` sets the next new-file line number to `c`; a line starting `+`
   (and not `+++`) is an added line; a line starting with a space advances the line number; a line starting
   `-` does not. Only added lines are matched. `line` is the line number in the new file.
3. Lines never matched:
   - in a test file: a path component equal to `tests`, `test`, `__tests__`, `benches`, `examples`,
     `fixtures` or `testdata`; or a file name starting `test_`, equal to `conftest.py`, or ending
     `_test.<ext>`, `.test.<ext>` or `.spec.<ext>`;
   - in a Rust file, everything after a line (added or context) containing `#[cfg(test)]` within the same
     file's hunks, and every hunk whose `@@` header text contains `mod tests`;
   - a line whose first non-blank characters are `//`, `#` (except `#[` and `#!` in Rust), `*` or `--`.
4. Patterns, by file extension. `pattern` is the name in the first column.

   | `pattern` | Extensions | An added line matches when it contains |
   |---|---|---|
   | `unwrap_or` | `rs` | `.unwrap_or(` |
   | `unwrap_or_default` | `rs` | `.unwrap_or_default(` |
   | `unwrap_or_else` | `rs` | `.unwrap_or_else(` |
   | `except-pass` | `py` | `except:`, `except Exception` or `except BaseException`, when the rest of that line after its `:` is `pass`, `continue` or starts with `return`, or when the next added line (blank lines skipped) is `pass`, `continue` or starts with `return` |
   | `get-default` | `py` | `.get(` or `getenv(` whose argument list has a comma at its top level (not inside nested brackets or a string) |
   | `or-literal` | `py` | ` or ` followed by `"`, `'`, `[`, `{`, `(` or a digit |
   | `nullish` | `js`, `jsx`, `mjs`, `ts`, `tsx` | `??` |
   | `or-literal` | `js`, `jsx`, `mjs`, `ts`, `tsx` | `\|\|` followed, after optional spaces, by `"`, `'`, a backtick, `[`, `{` or a digit |
   | `empty-catch` | `js`, `jsx`, `mjs`, `ts`, `tsx` | `catch` followed on the same line by `{}` or `{ }` (with or without a `(name)`) |
   | `or-true` | `sh`, `bash`, `yml`, `yaml` | `\|\| true` or `\|\| :` |
   | `param-default` | `sh`, `bash`, `yml`, `yaml` | `${` ... `:-` |

   A line gives at most one finding per pattern. Files with other extensions give none.
5. `flag`: `None` for no findings; otherwise
   `possible fallbacks added (<n>): <file>:<line> <pattern>, <file>:<line> <pattern>, ... : check each against the task`,
   listing the first three findings and, when there are more, ` and <k> more` before the colon. (The flag is
   cut to 200 characters by the existing `sanitize_flag` rule; apply the same cut here.)
6. In `run_review`, for a run that is not a research run: always run `git diff <base>..HEAD` in the
   worktree (today it runs only with `--diff`), keep printing `== diff` only with `--diff`, and after
   `review_checks::build` push `flag(&added_fallbacks(&diff))` when it is `Some`. If the `git diff` command
   cannot be run or exits non-zero, push the flag `fallback check not run: git diff failed: <reason>`
   instead: never skip the check silently. Research runs change no code and are not checked.

**Tests** (`crates/blyte-cli/tests/test_fallback_check.rs`). Each test passes a literal unified diff to
`added_fallbacks`. A helper `diff(path: &str, start: usize, lines: &[&str]) -> String` builds one file's
diff from lines already prefixed with `+`, `-` or a space.

1. `an_added_unwrap_or_default_is_found`: `src/a.rs`, hunk at line 10, lines
   `[" let x = 1;", "+let region = cfg.region.clone().unwrap_or_default();"]` gives exactly
   `[Finding { file: "src/a.rs", line: 11, pattern: "unwrap_or_default" }]`.
2. `a_removed_or_context_line_is_not_found`: the same call on a `-` line and on a context line gives none.
3. `rust_unwrap_or_and_unwrap_or_else_are_found`: one added line each; two findings with those patterns.
4. `a_test_file_is_ignored`: the diff of test 1 with paths `tests/a.rs`, `crates/x/tests/t.rs`,
   `src/test_a.py` and `web/a.test.ts` (each with a line that would match) gives none.
5. `a_rust_test_module_is_ignored`: `src/a.rs` with a context line `#[cfg(test)]` and then an added
   `.unwrap_or(0)` gives none; the same added line before the `#[cfg(test)]` line gives one.
6. `a_comment_is_ignored`: `+    // was .unwrap_or_default()` gives none.
7. `python_except_pass_is_found`: `src/a.py` with `+    except Exception:` then `+        pass` gives one
   `except-pass` at the line of `except`; `+    except ValueError:` then `+        raise` gives none; the
   one-line form `+    except: pass` gives one.
8. `python_get_with_a_default_is_found`: `+region = cfg.get("region", "EUR")` gives `get-default`;
   `+region = cfg.get("region")` gives none; `+v = d.get(key(a, b))` gives none;
   `+home = os.getenv("HOME", "/tmp")` gives `get-default`.
9. `python_or_literal_is_found`: `+name = name or "unknown"` gives `or-literal`; `+ok = a or b` gives none.
10. `typescript_patterns_are_found`: in `web/a.ts`, `+const r = cfg.region ?? "EUR";` gives `nullish`;
    `+const n = count || 0;` gives `or-literal`; `+if (a || b) {` gives none; `+} catch (e) {}` gives
    `empty-catch`.
11. `shell_patterns_are_found`: in `scripts/run.sh`, `+rm -f x || true` gives `or-true`;
    `+REGION="${REGION:-eu-west-1}"` gives `param-default`.
12. `a_deleted_file_and_an_unknown_extension_are_ignored`: a `+++ /dev/null` file, and `notes.txt` with
    `+a ?? b`, give none.
13. `line_numbers_follow_the_hunks`: two hunks in one file (`@@ -1,2 +1,3 @@` and `@@ -40,2 +41,3 @@`) with
    one matching added line in each, as the second line of its hunk, give lines 2 and 42.
14. `the_flag_lists_three_and_counts_the_rest`: five findings give a string starting
    `possible fallbacks added (5): ` that names the first three as `file:line pattern`, contains
    ` and 2 more`, and ends `: check each against the task`; `flag(&[])` is `None`.
15. `review_flags_a_fallback_the_run_added` (the wiring): set up a finished run the way
    `crates/blyte-cli/tests/test_review_base.rs` does, with the run's commit adding `src/a.rs` containing
    `let r = x.unwrap_or_default();`. `blyte review <id>` output has, in its `== checks` section, a line
    starting `possible fallbacks added (1): src/a.rs:`. The same run without `--diff` prints no `== diff`
    section.

Run: `cargo test -p blyte --test test_fallback_check`, then the full gate.

**Commit message:** `feat(review): flag fallbacks a run added`

---

## After the tasks

- Merge each chain in order by PR, re-running the gate after each rebase.
- Check on real runs before relying on the new research checks: re-run the quote check on recent verified
  reports in this repository and count new problems. A report that was sound and now fails is a false
  positive to fix before release.
- FB is a pointer for the reviewer. Count its flags against review verdicts for two weeks; drop a pattern
  that is mostly noise.
