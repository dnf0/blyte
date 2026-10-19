# Plan: claim, name and path checks for research runs (2026-10-19)

Spec: `docs/specs/2026-10-19-claim-check.md`. Tasks N1 and P1 are independent (different files) and
run in parallel; C1 runs after both are merged (it touches `run.rs` and `dispatch.rs` again).

### Task N1: name check: report prose may only name identifiers the run has seen

Files: `crates/blyte-agent/src/research_check.rs`, `crates/blyte-agent/src/run.rs`, new test
`crates/blyte-agent/tests/test_name_check.rs`, `docs/usage.md` (research section).

1. **In `research_check.rs`:** `pub fn identifiers(text: &str) -> impl Iterator<Item = &str>` yielding
   every `[A-Za-z_][A-Za-z0-9_]*` token of 3+ characters, and
   `pub fn unseen_names(report: &str, seen: &HashSet<String>) -> Vec<String>`:
   - Look only at backticked inline spans in prose: skip fenced code blocks (reuse how `check` /
     `claims` already tell prose from fences).
   - From each span take the code-like names: an identifier token that contains `_`, or has a
     lowercase letter followed later by an uppercase one (`camelCase`, `PascalCase` with an inner
     capital), or is directly followed by `(` in the span, or is joined to another identifier by
     `.` (`stale_cache.purge_for_project`: check each part). Plain words (`report`, `Gate`) and
     numbers are never names.
   - A name not in `seen` is a problem, formatted like the existing ones:
     ``report.md:{line}: `{name}` does not appear in anything this run read (the task, files or command output); name only what you have seen``.
     One problem per distinct name (first line it occurs on).
2. **In `run.rs`:** the `Runner` keeps `seen_names: RefCell<HashSet<String>>` (or the interior
   mutability the struct already uses), filled from the task goal at the start of `run_until` and
   from every tool result's content where it is pushed to `history` (`Message::ToolResult`, near the
   tool-execution loop), plus any `history` restored on resume. It is never cleared, so folding does
   not drop names. Only fill it when `self.research` (no cost to other runs).
3. **Wire it in:** `check_report_quotes` appends `unseen_names(report, &seen)` to its problems, so the
   name check blocks the gate like the quote check and runs on early stop too (that path already
   calls `check_report_quotes`). Keep the `MAX_PROBLEMS` cap after merging both lists.
4. **Tests** (`test_name_check.rs`, unit-level on `unseen_names`, plus one runner test copied from
   `crates/blyte-agent/tests/test_research_stop_check.rs`'s setup):
   - `MAX_CONCURRENT_SPATIAL_ACTIVITIES`, `stale_cache.purge_for_project(...)` and
     `spectra.overwritten_asset_attributes` in prose with a `seen` set lacking them → 3 problems
     (`spectra` alone is a plain word and not flagged; the dotted parts that are unseen are);
   - the same names inside a fenced block → no problems; names present in `seen` → none;
   - plain backticked words (`report.md` is a path: `report` and `md` are plain → not flagged),
     numbers and `--flags` → none;
   - runner test: a research run whose tool result contains `real_name` and whose report.md names
     `` `real_name` `` and `` `made_up_name` `` → `quote_problems` contains exactly the
     `made_up_name` problem.
5. **Docs:** one sentence in the research section of `docs/usage.md`.

Gate: the repository checks. Commit: `feat(research): report prose may only name identifiers the run has seen`

### Task P1: make unreadable paths loud (dispatch refusal, tool message, review line)

Files: `crates/blyte-cli/src/dispatch.rs`, `crates/blyte-agent/src/tools.rs`, new test
`crates/blyte-cli/tests/test_path_check.rs` (or add to an existing dispatch test file if one
already builds dispatch args without a provider), `docs/usage.md` (dispatch / `--allow-read` part).

1. **Dispatch check.** In `blyte dispatch`, after the task text is known (task file and/or the
   `--plan-task` section) and the read roots are collected (`config_allow_read` + `args.allow_read`,
   near `dispatch.rs:508-530`), and before the worktree is created: find absolute paths in the task
   text (`/` followed by non-whitespace, trimmed of trailing punctuation and quotes/backticks) that
   exist on disk and whose canonical path is outside the main checkout and outside every read root.
   - `--research`: exit with an error listing up to 5 of them and the fix, e.g.
     `blyte: the task names paths this run cannot read: <paths>. Add --allow-read <dir> for each (or --no-path-check to start anyway).`
   - Other dispatches: print the same as a warning to stderr and continue.
   - Add the `--no-path-check` flag (clap, next to `--allow-read`). Put the scan in a small pure
     function (`fn unreadable_paths(text: &str, readable: &[PathBuf]) -> Vec<PathBuf>`) so it is
     unit-tested without a dispatch.
2. **Tool message.** In `tools.rs`, `resolve_for_read`'s error (used by `read`, `glob`, `grep`)
   becomes: `path escapes the workspace: {raw}. This run can read the workspace{ and <roots>}. Do not look for another copy: if the task needs this path, say in your answer that it was not readable.`
   `resolve_in_workspace`'s own message (write tools) is unchanged. Existing tests match on
   `contains("escapes the workspace")`, so they keep passing; update any exact-match test.
3. **Review line.** In the `blyte review` printer's `== checks` section (`dispatch.rs`, near
   `println!("== checks")`), count log lines containing `path escapes the workspace` in
   `.wt/<id>.log`; when there are any, print
   `refused reads: N (outside the workspace and --allow-read): <first 3 distinct paths>`, following
   the section's existing style.
4. **Tests:** `unreadable_paths` with a temp dir: an existing path outside → returned; inside the
   checkout or a read root → not; a non-existent path → not; trailing `.`/`)`/backtick trimmed.
5. **Docs:** in `docs/usage.md` where `--allow-read` is described, one sentence on the refusal and
   `--no-path-check`.

Gate: the repository checks. Commit: `feat(dispatch): refuse research runs that name paths they cannot read`

### Task C1: claim checker in the research gate, one fix round

Files: new `crates/blyte-agent/src/claim_check.rs` (+ `lib.rs` module line), `crates/blyte-agent/src/run.rs`,
`crates/blyte-agent/examples/claim_check.rs`, `crates/blyte-cli/src/report.rs`,
`crates/blyte-cli/src/dispatch.rs`, new test `crates/blyte-agent/tests/test_claim_gate.rs`, `docs/usage.md`.

1. **Library module `claim_check.rs`.** Move from `examples/claim_check.rs` the evidence formatting
   (`format_evidence`), the check request (`build_check_request`), and the reply parsing
   (`extract_json_array`, `parse_verdict_reply`, `VerdictItem`) into the library, unchanged in
   behaviour, and make the example use them. Add:
   `pub struct ClaimCheck { pub checked: usize, pub unchecked: usize, pub contradicted: Vec<ClaimFlag>, pub not_enough_evidence: usize, pub parse_errors: usize, pub calls: u32, pub usage: Usage }`,
   `pub struct ClaimFlag { pub line: usize, pub text: String, pub reason: String }` (serde
   Serialize/Deserialize, `line` = the claim's first line in report.md; add it to `Claim` in
   `research_check.rs` if `Claim` lacks it), and
   `pub fn check_claims(provider: &dyn Provider, claims: &[Claim]) -> ClaimCheck`:
   at most 64 claims (rest counted `unchecked`), batches of 8 numbered 1..n within the batch, one
   retry of a batch whose reply fails to parse (then its claims count as `parse_errors`), all usage
   summed (including failed calls). Batches may run concurrently with `std::thread::scope` (up to
   4) if the provider type is `Sync`; otherwise sequentially.
2. **Runner switch.** `Runner` gets `claim_check: bool`, default `false`, with a builder method like
   the existing ones. `blyte dispatch --research` sets it unless the new `--no-claim-check` flag
   is given.
3. **In the gate** (`run_loop`, the `if passed && self.research` block): when the quote check
   passed and `self.claim_check`, read report.md, build `Sources` as `check_report_quotes` does,
   `research_check::claims(...)`, then `check_claims`. Add its usage to `report.usage` (so cost
   and `--max-cost` include it) and store `report.claim_check = Some(result)`.
   - If it found contradicted claims and no claim round has been fed back yet: set the round flag,
     `passed = false`, and `output` = `exit 1\n` + a list (`report.md:{line}: {text cut to 200 chars}\n  checker: {reason}`)
     and the line: `These claims do not match the lines they cite. Fix each claim, cite the lines that support it, or remove it.`
   - Otherwise the gate passes; remaining contradicted claims stay in `report.claim_check`.
4. **On early stop** (`run_until`, where the quote check on stop runs): if `self.claim_check` and
   report.md is non-empty, run the claim check too and store it (advisory; outcome unchanged).
   Add usage to `report.usage`.
5. **Report and review.** `RunReport.claim_check: Option<ClaimCheck>`; mirror in `RunRecord`
   (`report.rs`) with `#[serde(default, skip_serializing_if = "Option::is_none")]`. In `blyte review`
   next to the quote-check line: `claim check: N checked, K contradicted, E not enough evidence, P parse errors, U unchecked ($cost)`
   and the first 5 contradicted claims (line, text cut to 200, reason); add a `== checks` line when
   K > 0.
6. **Tests** (`test_claim_gate.rs`, scripted provider as in `test_research_stop_check.rs`; the
   claim-check calls consume scripted replies, so script them):
   - claim check on, report with one cited claim, checker replies `contradicted` → gate fails once
     with the claim in the fed-back message; model edits; checker replies `supported` → `Verified`,
     `claim_check.contradicted` empty;
   - checker replies `contradicted` both times → second gate passes (`Verified`), `contradicted` has 1;
   - unparseable checker reply then a valid one → one retry, `parse_errors == 0`, `calls == 2`;
   - claim check off (default) → no checker calls (scripted replies unchanged from today's tests);
   - unit tests in `claim_check.rs` for batching (17 claims → 8, 8, 1) and the 64 cap.
7. **Docs:** in the research section of `docs/usage.md`, a short paragraph: the claim checker, one
   fix round, what review shows, `--no-claim-check`.

Gate: the repository checks. Commit: `feat(research): model claim check in the research gate, one fix round`
