# Research guardrails: plan

**Date:** 2026-11-18. **Spec:** `docs/specs/2026-11-18-research-guardrails.md`.

Three tasks. RG1 and RG2 each add one new module with pure functions and its tests, and touch no
shared file except one `pub mod` line each in `crates/blyte-cli/src/lib.rs`. RG3 wires both into
`dispatch.rs` and `cli.rs`. The skill and usage-guide text (decision 6) is written directly, not
dispatched.

## Facts checked (at `8a6c0da`)

- `crates/blyte-cli/src/dispatch.rs`: the task text is loaded into `task_content` (a file, or a plan
  section); a missing task prints `error: give a task file, or --plan with --plan-task` and returns 2;
  the path check follows under `if !args.no_path_check` and uses `unreadable_paths(text, readable)`,
  which splits the text on whitespace and quote/bracket characters.
- `crates/blyte-cli/src/cli.rs`: `no_path_check: bool` and `no_claim_check: bool` are dispatch flags.
- `crates/blyte-cli/src/provenance.rs` writes the banner
  `> **Not verified.** This research run ended as ...` as the first line of an unverified report copy.
- Review (`dispatch.rs`, after `crate::review_checks::build(...)`) pushes strings onto `checks.flags`,
  for example `read loop: ...`; the flags are printed under `== checks` and saved to
  `.wt/<id>.checks.json`.
- The run report `.wt/<id>.report.json` has `turn_usage`: one object per turn with `input` and `cached`
  token counts. `.wt/<id>.start.json` has `"research": true` for a research run.
- The run log `.wt/<id>.log` has one line per tool call, such as
  `● read /abs/path/file.rs ->      1\t...` and `● read /abs/path/file.rs (L1-L800) ->      1\t...`.

### Task RG1: the research brief check

**Goal:** pure functions that check a research question has the four brief sections and names no
unverified report, plus the brief template. Why: research runs on vague or broad questions waste
their budget, and an unverified report passed to a later run spreads its mistakes.

**Depends on:** none. **Siblings:** RG2 adds `research_breadth.rs` separately and RG3 wires both into
`dispatch.rs` and `cli.rs`; neither is in this worktree. Do not edit `dispatch.rs`, `cli.rs` or
`provenance.rs` here.

**Files:**
- create `crates/blyte-cli/src/research_brief.rs`
- `crates/blyte-cli/src/lib.rs`: add `pub mod research_brief;` only

**What to build** (all in `research_brief.rs`):

```rust
/// The first words of the line blyte puts at the top of a research report that did not pass its checks.
pub const NOT_VERIFIED_PREFIX: &str = "> **Not verified.**";

/// The sections a research question must have, in order.
pub const SECTIONS: [&str; 4] = ["Objective", "Start from", "Out of scope", "Answer format"];

/// A question to fill in: a title line and the four sections, each with a one-line prompt.
pub const TEMPLATE: &str = "...";

/// The sections of `SECTIONS` that `question` has no heading for, in `SECTIONS` order.
pub fn missing_sections(question: &str) -> Vec<&'static str>

/// Markdown files named in `question` that exist and start with `NOT_VERIFIED_PREFIX`.
pub fn unverified_inputs(question: &str, base: &std::path::Path) -> Vec<std::path::PathBuf>
```

- A heading is a line that, after leading spaces, starts with one to six `#` characters and a space.
  A heading matches a section when its text, with leading and trailing spaces and a trailing `:`
  removed, starts with the section name, compared without regard to case (`## Objective`,
  `### objective:`, `## Start from (files)` all match). A heading inside a fenced code block
  (between lines starting with three backticks) does not count.
- `TEMPLATE` is:

  ```text
  # Question: <one line>

  ## Objective
  <what you need to know, and what you will do with the answer>

  ## Start from
  <the files or directories to read first, as absolute paths>

  ## Out of scope
  <what not to read or answer; other questions being asked separately>

  ## Answer format
  <what the report must contain: a table, paths with line numbers, a recommendation; and a length limit>
  ```

  `missing_sections(TEMPLATE)` is empty.
- `unverified_inputs`: split `question` on whitespace and on the characters
  `` ` `` `"` `'` `(` `)` `[` `]` `{` `}` `<` `>`; from each piece remove trailing `.`, `,`, `;` and
  `:`; keep pieces that end in `.md`. A relative piece is joined to `base`. Keep a path when the file
  exists and its content starts with `NOT_VERIFIED_PREFIX`. Each path once, in the order first seen.
  A piece that names no existing file is not an error here (the path check covers unreadable paths).

**Tests** (unit tests in `research_brief.rs`; run with `cargo test -p blyte --lib research_brief`, or
the package name `crates/blyte-cli/Cargo.toml` gives):

1. `the_template_has_every_section`: `missing_sections(TEMPLATE)` is empty.
2. `missing_sections_are_named_in_order`: a question with only `## Objective` and `## Answer format`
   gives `["Start from", "Out of scope"]`; an empty question gives all four.
3. `headings_match_without_case_and_with_extras`: `### objective:`, `# START FROM (files)`,
   `## Out of scope`, `#### Answer format: a table` together give no missing sections.
4. `a_heading_in_a_code_block_does_not_count`: `## Objective` only inside a fenced block is missing.
5. `a_line_that_is_not_a_heading_does_not_count`: `Objective: find X` and `**Objective**` are missing.
6. `an_unverified_report_is_found`: in a temp dir, `a.report.md` starts with
   `> **Not verified.** This research run ended as` and `b.report.md` starts with `# Findings`; a
   question naming both by absolute path (one in backticks, one followed by a full stop) returns only
   `a.report.md`.
7. `a_relative_path_resolves_against_base`: the question names `notes/a.md`, which exists under
   `base` and starts with the prefix: returned as `base/notes/a.md`.
8. `a_missing_file_and_a_repeat_are_not_reported_twice`: a named `.md` path that does not exist gives
   nothing; the same unverified file named twice is returned once.

**Constraints:** no fallbacks (nothing is guessed: a section is present or it is named as missing).
Existing tests pass unmodified. Not a long-running job. No new dependencies.

**Commit message:** `feat(research): a brief check for research questions`

### Task RG2: review's breadth flag and split suggestion

**Goal:** pure functions that tell a reviewer when a research run was too broad and where it spent
its reads. Why: broad questions are slow and fail; the reviewer needs to see that from the run's own
record.

**Depends on:** none. **Siblings:** RG1 adds `research_brief.rs` separately and RG3 wires both into
`dispatch.rs`; neither is in this worktree. Do not edit `dispatch.rs` or `review_checks.rs` here.

**Files:**
- create `crates/blyte-cli/src/research_breadth.rs`
- `crates/blyte-cli/src/lib.rs`: add `pub mod research_breadth;` only

**What to build** (all in `research_breadth.rs`):

```rust
/// A prompt above this many tokens (fresh plus cached input) counts as large.
pub const LARGE_PROMPT_TOKENS: u64 = 100_000;
/// A run with at least this many large-prompt turns is flagged as broad.
pub const BROAD_TURNS: usize = 10;
/// Most directories a split suggestion lists.
pub const MAX_DIRS: usize = 8;

/// (turns with a prompt over `LARGE_PROMPT_TOKENS`, turns in all) from a run report's `turn_usage`.
/// `Err` names what is wrong when `turn_usage` is missing or an entry lacks `input` or `cached`.
pub fn large_prompt_turns(report: &serde_json::Value) -> Result<(usize, usize), String>

/// The flag for a broad run, or `None` when fewer than `BROAD_TURNS` turns were large.
pub fn breadth_flag(large: usize, total: usize) -> Option<String>

/// Directories the run read files from, with the number of distinct files read in each,
/// most files first, then by name; at most `MAX_DIRS`.
pub fn read_dirs(log: &str, root: &std::path::Path) -> Vec<(String, usize)>

/// The flag for a run that ran out of budget, or `None` when `dirs` is empty.
pub fn split_flag(dirs: &[(String, usize)]) -> Option<String>
```

- A turn is large when `input + cached > LARGE_PROMPT_TOKENS`.
- `breadth_flag(12, 40)` is
  `broad research run: 12 of 40 turns had prompts over 100K tokens; narrower questions run faster and fail less`.
- `read_dirs`: a read line starts (after leading spaces) with `● read `; the path is the text up to
  the first ` -> `, with a trailing ` (L<digits>-L<digits>)` removed when present. A line without
  ` -> ` is skipped. The directory is the path's parent, shown relative to `root` when the path is
  under it (`.` for a file directly in `root`) and as the absolute parent otherwise. The same file
  read twice counts once.
- `split_flag(&[("crates/a/src".into(), 12), ("docs".into(), 3)])` is
  `ran out of budget: split the question; files read by directory: crates/a/src (12), docs (3)`.

**Tests** (unit tests in `research_breadth.rs`):

1. `large_turns_are_counted`: `turn_usage` of three turns with `(input, cached)` of `(60000, 50000)`,
   `(100000, 0)` and `(1, 100000)` gives `(2, 3)` (exactly 100,000 is not large).
2. `a_report_without_turn_usage_is_an_error`: `{}` gives `Err` whose text contains `turn_usage`; an
   entry without `cached` gives `Err` whose text contains `cached`.
3. `the_breadth_flag_needs_ten_large_turns`: `breadth_flag(9, 40)` is `None`; `breadth_flag(10, 40)`
   is `Some` and equals the sentence above with `10 of 40`.
4. `read_dirs_groups_distinct_files`: a log with reads of `/r/crates/a/src/x.rs` (twice, once with
   ` (L1-L800)`), `/r/crates/a/src/y.rs`, `/r/docs/u.md`, `/r/README.md` and `/other/z.rs`, plus a
   `● bash ls -> exit 0` line and a `● read /r/broken` line with no ` -> `, with root `/r`, gives
   `[("crates/a/src", 2), (".", 1), ("/other", 1), ("docs", 1)]`.
5. `read_dirs_lists_at_most_eight`: reads in ten different directories give eight entries.
6. `the_split_flag_lists_directories`: equals the sentence above; `split_flag(&[])` is `None`.

**Constraints:** no fallbacks (a report without the fields is an error naming the field, not a zero
count). Existing tests pass unmodified. Not a long-running job. No new dependencies.

**Commit message:** `feat(review): breadth and split checks for research runs`

### Task RG3: wire the research checks into dispatch and review

**Goal:** `blyte dispatch --research` applies the brief check and prints the template; `blyte review`
shows the breadth flag and the split suggestion.

**Depends on:** RG1, RG2 (both merged into the base branch). **Siblings:** none.

**Files:**
- `crates/blyte-cli/src/cli.rs`
- `crates/blyte-cli/src/dispatch.rs`
- `crates/blyte-cli/src/provenance.rs` (a test only)
- an integration test file under `crates/blyte-cli/tests/` (new, or the existing dispatch tests)

**What to build:**

1. `cli.rs`: a dispatch flag next to `no_path_check`:
   `#[arg(long = "no-brief-check")] pub no_brief_check: bool` with help
   `Start a research run even if the question lacks the brief sections or names an unverified report`.

2. `dispatch.rs`, research runs only (`args.research`):
   - when no task is given (the branch that prints
     `error: give a task file, or --plan with --plan-task`): for a research run also print to stderr
     `A research question needs these sections:` followed by a blank line and
     `research_brief::TEMPLATE`. Still return 2.
   - after `task_content` is loaded and before the path check, unless `args.no_brief_check`:
     - if `research_brief::missing_sections(&task_content)` is not empty, print to stderr
       `blyte: the research question has no section for: <names joined with ", ">. Add each as a heading (the template: blyte dispatch <id> --research), or --no-brief-check to start anyway.`
       and return 2;
     - if `research_brief::unverified_inputs(&task_content, &root)` is not empty, print to stderr
       `blyte: the question names a report that did not pass blyte's checks: <paths joined with ", ">. Use a verified report, or --no-brief-check to start anyway.`
       and return 2.
   - Both checks run before any worktree, branch or `.wt/<id>.*` file is created.
   - A non-research dispatch is not checked.

3. `dispatch.rs`, review: for a research run (`.wt/<id>.start.json` has `"research": true`; if
   review already knows whether the run is research, use that), after the existing flags are pushed:
   - `research_breadth::large_prompt_turns(&report_val)`: on `Ok((large, total))`, push
     `breadth_flag(large, total)` when it is `Some`; on `Err(e)`, push the flag
     `breadth check not run: <e>`;
   - when the outcome is `budget_exhausted`: read `.wt/<id>.log`; push
     `split_flag(&read_dirs(&log, &root))` when it is `Some`; when the log cannot be read, push
     `split suggestion not run: <the error>`.
   A non-research run gets neither flag.

4. `provenance.rs`: a unit test `the_banner_starts_with_the_prefix_the_brief_check_looks_for` that
   builds the banner the module writes and asserts it starts with
   `crate::research_brief::NOT_VERIFIED_PREFIX`.

**Tests** (integration, in the style of the existing dispatch tests in `crates/blyte-cli/tests/`,
which run the binary in a temp git repository; none of these may start a model run):

1. `research_dispatch_without_a_question_prints_the_template`: `blyte dispatch R1 --research` exits
   2; stderr contains `## Start from` and `## Answer format`.
2. `research_dispatch_names_missing_sections`: a question file with only `## Objective` exits 2;
   stderr contains `Start from, Out of scope, Answer format`; no `.wt/R1.task.md` and no `.wt/R1`
   exist afterwards.
3. `research_dispatch_refuses_an_unverified_report`: a question with all four sections that names a
   file starting with `> **Not verified.**` exits 2; stderr contains that file's path and
   `did not pass blyte's checks`.
4. `no_brief_check_gets_past_both`: the question from test 2 with `--no-brief-check` does not print
   `has no section for` (make the run stop at a later, cheap point, as the existing tests do, for
   example with no provider key in the environment; assert on stderr, not on a model run).
5. `an_implementation_dispatch_is_not_brief_checked`: the same question file without `--research`
   does not print `has no section for`.
6. `review_flags_a_broad_research_run`: with `.wt/R1.start.json` (`"research": true`), a
   `.wt/R1.report.json` whose `turn_usage` has 12 turns of `input: 120000, cached: 0`, outcome
   `budget_exhausted`, and a `.wt/R1.log` with reads in two directories: `blyte review R1` prints,
   under `== checks`, a line starting `broad research run: 12 of 12 turns` and a line starting
   `ran out of budget: split the question; files read by directory:`. Use whatever other `.wt/R1.*`
   files the existing review tests need.
7. `review_does_not_flag_a_narrow_or_non_research_run`: the same with 3 large turns and outcome
   `verified` prints neither line; the 12-turn report without `"research": true` prints neither.

**Constraints:** no fallbacks (each refusal names what is missing; a check that cannot run says so in
a flag instead of passing silently). Existing tests pass unmodified, except that a test which
dispatches a research question without the sections may add `--no-brief-check` or the sections: say
which in the final message. Not a long-running job.

**Commit message:** `feat(research): dispatch checks the brief and review flags broad runs`
