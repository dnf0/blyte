# Plan: run the research quote check when a run stops early (2026-10-18)

## Why

The research quote check (`check_report_quotes` in `crates/blyte-agent/src/run.rs`) runs only
inside the gate, and the gate runs only when the model stops calling tools. A research run that
runs out of time, turns or cost returns `Outcome::BudgetExhausted` from `run_loop` without ever
checking `report.md`, and `blyte dispatch` still copies that report to `.wt/<id>.report.md`.
The R-V5b7 rerun (2026-10-18, see `docs/specs/2026-10-16-research-quote-check.md`) did exactly
this: out of time, 15 of 28 code blocks invented, nothing flagged.

Decision (user, 2026-10-18): check the report whenever a research run stops without a passing
gate, and show the result. The outcome itself does not change (still `budget_exhausted`).

### Task Q1: quote-check the report of a research run that ends without a passing gate

Files: `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/report.rs`,
`crates/blyte-cli/src/dispatch.rs`, a new test `crates/blyte-agent/tests/test_research_stop_check.rs`,
`docs/usage.md` (the research-run section).

1. **Run report field.** Add to `RunReport` (`run.rs`) `pub quote_problems: Option<Vec<String>>`:
   `None` when the check did not run, `Some(vec![])` when it ran and passed, `Some(problems)`
   otherwise (at most `research_check::MAX_PROBLEMS`, as `check` already caps). Mirror it in
   `RunRecord` (`crates/blyte-cli/src/report.rs`) with `#[serde(default, skip_serializing_if = "Option::is_none")]`
   so old report JSON still reads, mapped from the run report like `truncated_turns` is.
2. **When it runs.** In `Runner::run_until` (`run.rs`), after `self.run_loop(...)` returns: if
   `self.research`, the outcome is not `Outcome::Verified`, and `report.md` in the workspace exists
   and is non-empty (read it through `self.fs` as `run_loop` does), set
   `report.quote_problems = Some(self.check_report_quotes())`. Do not change the outcome. Also
   record it inside the gate path: where the gate's quote check already runs in `run_loop`, store
   its result in `report.quote_problems` as well, so a verified run shows `Some(vec![])`.
3. **Shown in `blyte review`.** In the `== report` section of the review printer
   (`crates/blyte-cli/src/dispatch.rs`, near `println!("== report")`), when `quote_problems` is
   `Some` and non-empty, print `quote check: FAILED, N problems (the report's quotes do not match the code)`
   followed by the first 5 problems, each cut to 200 characters; when `Some` and empty, print
   `quote check: passed`. Add a `== checks` warning line too when it failed (that section lists
   things the reviewer must look at; follow its existing style).
4. **Tests** in `test_research_stop_check.rs`, with a scripted provider like the research tests in
   `crates/blyte-agent/tests/test_research_reminder.rs` (copy its setup, including how it builds a
   research runner and a workspace file):
   - a research run whose provider keeps calling tools until `max_turns` runs out, having written
     a `report.md` with a code block cited to a file whose lines do not match: outcome is
     `BudgetExhausted` and `quote_problems` is `Some` and non-empty;
   - the same with a matching quote: `Some(vec![])`;
   - a non-research run that runs out of turns: `quote_problems` is `None`;
   - a research run with no `report.md`: `None`.
   Add a review-printer test if the printer has tests nearby (grep `crates/blyte-cli/tests` for
   `== report`); otherwise skip.
5. **Docs**: one sentence in `docs/usage.md` where research runs and the quote check are
   described: a run that stops early still has its report checked, and `blyte review` shows the
   result.

Gate: the repository checks (`blyte.toml`). Commit message:
`fix(research): quote-check the report when a research run stops without a passing gate`
