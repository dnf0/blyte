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

Task C1 (2026-10-19) re-read the same five large files every turn for 250 turns without an edit
(folding dropped them each time) and was stopped. It is split into C1a then C1b, each with the
line ranges to read. **Read only the ranges named; never read a whole large file** (`run.rs`,
`dispatch.rs`, `cli.rs`, `task_runner.rs` are 500-1700 lines). Use `grep -n` to find anything else.
`Runner` already has `pub claim_check: bool` (false everywhere; commit `1d16018`).

### Task C1a: `claim_check` library module

Files: new `crates/blyte-agent/src/claim_check.rs`, `crates/blyte-agent/src/lib.rs` (one `pub mod`
line), `crates/blyte-agent/examples/claim_check.rs`, `crates/blyte-agent/src/research_check.rs`
(only if `Claim` needs a `line` field).

1. Move from `examples/claim_check.rs` into the new module, behaviour unchanged: `VerdictItem`
   (line 31), `format_evidence` (129), `build_check_request` (165, generalise its argument from the
   example's `PoolClaim` to `&[(usize, &Claim)]`, id and claim), `extract_json_array` (194),
   `parse_verdict_reply` (215). Make them `pub`; the example imports them from
   `blyte_agent::claim_check` and keeps its own seeding and table code.
2. Add `ClaimFlag { line: usize, text: String, reason: String }` and
   `ClaimCheck { checked, unchecked, contradicted: Vec<ClaimFlag>, not_enough_evidence, parse_errors: usize, calls: u32, usage: Usage }`
   (serde Serialize, Deserialize, Default, Clone, Debug). `line` is the claim's first line in
   report.md: if `research_check::Claim` (see `pub struct Claim`, about line 48, and `fn claims`,
   about line 780) has no line number, add `pub line: usize` and set it where the claim unit is
   flushed (`flush_claim_unit`).
3. `pub fn check_claims(provider: &dyn Provider, claims: &[Claim]) -> ClaimCheck`: first 64 claims
   (the rest count as `unchecked`), batches of 8 with ids 1..=n in the batch, sequential calls. A
   reply that fails to parse (or a provider error) is retried once; if it fails again the batch's
   claims count as `parse_errors`. Sum usage over all calls, including failed ones
   (`ProviderError::NoAnswer` carries usage; see how `run.rs` adds it, `grep -n NoAnswer`).
   `contradicted` verdicts become `ClaimFlag`s; `not_enough_evidence` is counted.
4. Unit tests at the bottom of `claim_check.rs`, with a tiny scripted `Provider` (copy the pattern
   from `crates/blyte-agent/tests/test_research_stop_check.rs`, first 60 lines): 17 claims → 3
   calls (8, 8, 1); 70 claims → 64 checked, 6 unchecked; an unparseable reply then a valid one →
   `calls == 2`, `parse_errors == 0`; two unparseable → that batch's claims in `parse_errors`;
   a `contradicted` verdict → one flag with the claim's line and the reason.

Gate: the repository checks. Commit: `feat(research): claim_check library module`

### Task C1b: run the claim check in the research gate, one fix round (after C1a)

Files: `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/report.rs`,
`crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/task_runner.rs`,
new test `crates/blyte-agent/tests/test_claim_gate.rs`, `docs/usage.md` (research section only).

1. **run.rs.** `RunReport` (`pub struct RunReport`, about line 107; `quote_problems` at about 136):
   add `pub claim_check: Option<claim_check::ClaimCheck>` next to `quote_problems`, serialized the
   same way. Add a helper `fn run_claim_check(&self) -> Option<ClaimCheck>` next to
   `check_report_quotes` (about line 620; read 600-680): `None` unless `self.claim_check` and
   report.md is non-empty; build `Sources` exactly as `check_report_quotes` does, call
   `research_check::claims`, then `claim_check::check_claims(self.provider, ...)`.
   - **Gate** (read 1035-1070; the block starting `if passed && self.research`): after the quote
     check, if still `passed`, call `run_claim_check`; add its usage into `report.usage`
     (input, cached, output, thinking) and store it in `report.claim_check`. If it has
     contradicted claims and a local `claim_round_done` (declared before the turn loop) is false:
     set it, `passed = false`, and `output` = `exit 1\n` + one entry per flag
     (`report.md:{line}: {text cut to 200 chars}\n  checker: {reason}`) + the line
     `These claims do not match the lines they cite. Fix each claim, cite the lines that support it, or remove it.`
     Otherwise leave `passed` as is.
   - **Early stop** (read 715-735; where `report.quote_problems = Some(problems)` is set after
     `run_loop`): also run `run_claim_check`, add its usage, store it. Outcome unchanged.
2. **report.rs.** Mirror `quote_problems` (lines about 99, 159, 213): field
   `claim_check: Option<serde_json::Value>` or the typed struct, `#[serde(default, skip_serializing_if = "Option::is_none")]`, mapped from the run report.
3. **cli.rs / task_runner.rs / dispatch.rs: the switch.** The solver (`task_runner.rs`, config
   `research` at about lines 63, 100, 464) gets `claim_check: bool` from a new solver CLI flag
   `--claim-check` (next to `research`/`allow_read` in the solver's args, `cli.rs` about line 446)
   and sets `Runner.claim_check` (line 465, currently `claim_check: false`). `blyte dispatch`
   (`dispatch.rs` about line 557, `cmd.arg("--research")`) also passes `--claim-check` for research
   unless the new dispatch flag `--no-claim-check` is given (add it next to `no_path_check`,
   `cli.rs` about line 182).
4. **Review** (`dispatch.rs`, read 1010-1095, where `quote_problems` is printed): after the
   quote-check line print
   `claim check: N checked, K contradicted, E not enough evidence, P parse errors, U unchecked`
   and the first 5 contradicted claims (`report.md:{line}: {text cut to 200}` and the reason on the
   next line); add a `checks.flags` line `claim check: K contradicted claims` when K > 0, as the
   quote check does.
5. **Tests** (`test_claim_gate.rs`; copy the scripted provider and research runner setup from
   `test_research_stop_check.rs` and set `claim_check: true`). The checker's calls take scripted
   replies in order with the worker's, so script them:
   - checker says `contradicted` → gate fails once and the next request contains the claim; worker
     rewrites report.md; checker says `supported` → `Verified`, `claim_check` has no flags;
   - checker says `contradicted` twice → second gate passes, `Verified`, one flag kept;
   - `claim_check: false` → no checker call (scripted replies as before).
6. **Docs:** a short paragraph in the research section of `docs/usage.md`: the claim checker, one
   fix round, what review shows, `--no-claim-check`.

Gate: the repository checks. Commit: `feat(research): model claim check in the research gate, one fix round`
