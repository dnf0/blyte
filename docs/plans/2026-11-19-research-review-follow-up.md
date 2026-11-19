# Research review follow-up: plan

**Date:** 2026-11-19. **Spec:** `docs/specs/2026-11-18-research-guardrails.md` (decisions 1, 4 and 5).

Two gaps found after v0.44.0:

1. `blyte review <id>` stops with `error: worktree ... does not exist` once a run's worktree is
   removed. A research run's worktree is removed as soon as its report has been copied out, so the
   breadth flag and the split suggestion are lost for most research runs.
2. The spec says to compare runs whose question had the brief sections with runs whose question did
   not, after about 20 runs. Nothing records which is which.

Two tasks, in parallel. Both change `crates/blyte-cli/src/dispatch.rs`, in different functions
(`run_review` and `run_dispatch`), so each is told to stay inside its function.

## Facts checked (at `2206950`, v0.44.0)

- `run_review` (`dispatch.rs`, about line 1291): after the `--verdict` branch it sets
  `let wt = root.join(".wt").join(&args.id);` and returns 2 with `error: worktree {} does not exist`
  when it is missing. It then runs `git` in `wt` in nine places (commits, changed files, tree state,
  diff) and passes `[root, wt]` as the allowed roots of the read check.
- `is_research` there is `crate::clean::is_research(&root, &args.id) || <.wt/<id>.report.md exists>`.
- The research flags (quote check, claim check, read loop, breadth, split) are built from
  `.wt/<id>.report.json` and `.wt/<id>.log`, which stay after the worktree is removed.
- `run_dispatch` builds `start_json` (about line 1006) and sets `start_json["research"] = true` for a
  research run; the brief check (about line 365) calls
  `crate::research_brief::missing_sections(&task_content)` only when `--no-brief-check` is absent.

### Task RV1: review a research run after its worktree is removed

**Goal:** `blyte review <id>` works for a research run whose worktree is gone, from the files saved
beside it. Why: that is the normal state of a finished research run, and the review flags for
research runs are only useful if they can be seen then.

**Depends on:** none. **Siblings:** RB1 adds a field to the start record in `run_dispatch`; it is not
in this worktree. Change `dispatch.rs` only inside `run_review` (and helpers you add for it); do not
touch `run_dispatch`.

**Files:**
- `crates/blyte-cli/src/dispatch.rs` (`run_review` only)
- `crates/blyte-cli/tests/test_research_guardrails.rs` (add tests; its helpers show how to build a
  repository with saved run files)

**What to build:**

- When `.wt/<id>` does not exist and the run is a research run (`is_research`, as computed today)
  and `.wt/<id>.report.json` exists: do not stop. Review from the saved files:
  - every section that needs the worktree (`== commits`, `== files`, `== tree`, and the diff of
    `--diff`) prints its heading followed by the one line `(worktree removed)`, and no `git` command
    is run in the missing directory;
  - `== checks` holds every flag that is built from the saved files, exactly as when the worktree
    exists: the outcome flag, the quote check, the claim check, the read loop, the breadth flag and
    the split suggestion. Flags that only make sense with a worktree (for example `no files changed`,
    files not named in the task, commit messages) are left out;
  - `== report` and the report text print as they do today;
  - `.wt/<id>.checks.json` is written as it is today;
  - the exit code is what it would be with the worktree present.
- When `.wt/<id>` does not exist and the run is a research run but `.wt/<id>.report.json` does not
  exist: return 2 with
  `error: worktree <path> does not exist and <path of report.json> is missing: nothing to review`.
- When `.wt/<id>` does not exist and the run is not a research run: unchanged (the existing error).
- When the worktree exists: output unchanged.
- `--verdict` already works without a worktree; leave it.

**Tests** (in `test_research_guardrails.rs`; none may start a model run):

1. `review_works_for_a_research_run_without_its_worktree`: `.wt/R1.start.json` with
   `"research": true`, `.wt/R1.report.json` with outcome `budget_exhausted` and 12 `turn_usage`
   entries of `input: 120000, cached: 0`, a `.wt/R1.log` with reads in two directories, and no
   `.wt/R1` directory: `blyte review R1` does not print `does not exist`; stdout has a line starting
   `! broad research run: 12 of 12 turns`, a line starting `! ran out of budget: split the question`,
   and `(worktree removed)` under `== commits`; it does not contain `no files changed`.
2. `review_gives_the_same_research_flags_with_and_without_the_worktree`: the same files, reviewed
   once with a `.wt/R1` worktree present (as the existing test `review_flags_a_broad_research_run`
   sets it up) and once after removing it: the `broad research run` and `ran out of budget` lines
   are identical in both outputs.
3. `review_without_a_worktree_or_a_report_names_both`: research start record, no worktree, no
   `.wt/R1.report.json`: exit 2 and stderr contains `nothing to review` and `R1.report.json`.
4. `review_of_an_implementation_run_still_needs_its_worktree`: a start record without `research`, a
   report.json, no worktree and no `.wt/R1.report.md`: exit 2 and stderr contains `does not exist`.

**Constraints:** no fallbacks (a missing report is an error that names the file; a section that
cannot be produced says `(worktree removed)` instead of printing an empty section). Existing tests
pass unmodified. Not a long-running job. No new dependencies.

**Commit message:** `fix(review): a research run can be reviewed after its worktree is removed`

### Task RB1: record which brief sections a research question lacked

**Goal:** the start record of a research run says which brief sections its question lacked, so runs
with and without a full brief can be compared later. Why: the brief check is a warning until that
comparison has been made, and today nothing collects the data.

**Depends on:** none. **Siblings:** RV1 changes `run_review`; it is not in this worktree. Change
`dispatch.rs` only inside `run_dispatch`; do not touch `run_review`.

**Files:**
- `crates/blyte-cli/src/dispatch.rs` (`run_dispatch` only)
- `crates/blyte-cli/tests/test_research_guardrails.rs` (add tests)
- `docs/reports.md` (one line where the start record's fields are listed; if the start record is
  documented elsewhere in `docs/`, add the line there instead and say where)

**What to build:**

- For every research dispatch, compute `crate::research_brief::missing_sections(&task_content)` once,
  whether or not `--no-brief-check` is given (the flag hides the warning; it does not stop the
  record).
- Set `start_json["brief_missing"]` to the JSON array of the missing section names in
  `SECTIONS` order: `[]` when the question has all four.
- A non-research dispatch has no `brief_missing` field.
- The warning and the unverified-report refusal behave as today.

**Tests** (in `test_research_guardrails.rs`, in the style of
`research_dispatch_warns_about_missing_sections_and_goes_on`, which dispatches with no provider key
so that no model run starts; if `.wt/<id>.start.json` is not written on that path, find the existing
test that reads a start record and use its way of getting one, and say which in the final message):

1. `the_start_record_lists_missing_brief_sections`: a question with only `## Objective`:
   `.wt/R1.start.json` has `brief_missing` equal to
   `["Start from", "Out of scope", "Answer format"]`.
2. `the_start_record_has_an_empty_list_for_a_full_brief`: a question with all four sections:
   `brief_missing` is `[]`.
3. `no_brief_check_still_records_the_missing_sections`: the question of test 1 with
   `--no-brief-check`: `brief_missing` is the same three names and stderr has no `has no section for`.
4. `an_implementation_dispatch_has_no_brief_missing_field`: the same question file dispatched without
   `--research` (with `--gate true`): the start record has no `brief_missing` key.

**Constraints:** no fallbacks (the field is always present for a research run; absent means "not a
research run" or "dispatched before this version", never "unknown"). Existing tests pass unmodified.
Not a long-running job. No new dependencies.

**Commit message:** `feat(research): the start record lists the brief sections a question lacked`

## Check on real runs (2026-11-19)

With the worktrees of R-DA and R-DC already removed, `blyte review` printed their flags from the saved
files: R-DA `broad research run: 50 of 56 turns` and the split suggestion (`crates/blyte-cli/src (25)`,
`crates/blyte-agent/src (11)`, `crates/blyte-provider/src (7)`, ...); R-DC `42 of 47 turns`. An
implementation run without its worktree (A1) still gives `worktree ... does not exist`.
