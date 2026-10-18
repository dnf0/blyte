# Plan: measure a model claim checker on saved research reports (2026-10-18)

## Why

The research quote check (`crates/blyte-agent/src/research_check.rs`, shipped in 0.32.3) proves a
report's cited lines and quoted code match the files. It does not check that the code supports the
conclusion drawn from it. `docs/specs/2026-10-16-research-quote-check.md` ("Next, not built") says to
measure a model call per claim on saved reports before building it into runs. This plan builds
the measuring tool only; nothing changes in how runs are checked.

What we want to learn, on 10-15 saved reports:
- **Recall:** of claims deliberately made false (seeded), how many does the checker flag?
- **Flag rate on real claims:** how many of the reports' own claims it flags (then a person checks
  a sample to split real errors from false alarms).
- **Cost and time** per report, so a later decision can weigh it against a run's cost.

### Task C1: claim extraction in `research_check`, and a `claim_check` example that runs the experiment

Files: `crates/blyte-agent/src/research_check.rs`, new `crates/blyte-agent/examples/claim_check.rs`,
new `crates/blyte-agent/tests/test_claims.rs`.

1. **`pub fn claims(report: &str, src: &Sources<'_>) -> Vec<Claim>`** in `research_check.rs`, reusing
   the existing citation parsing (`citations`, `named_path`, `lead_in_lines`) and path resolution
   (`Resolver`) rather than new regexes.
   - A claim is one prose unit of the report that cites code: a paragraph, or a single list item,
     outside fenced code blocks, that contains at least one citation which resolves to an existing
     file. Headings are not claims. A fenced code block that directly follows the claim (only
     blank lines between) belongs to it as `quoted`.
   - `pub struct Claim { pub id: usize, pub text: String, pub quoted: Option<String>, pub evidence: Vec<Evidence> }`,
     `pub struct Evidence { pub path: String, pub start: usize, pub end: usize, pub lines: String }`
     where `lines` is the cited range widened by 5 lines each side (clamped to the file), each
     line prefixed with its 1-based number and a tab. Cap each evidence block at 120 lines (keep the
     cited range, trim the context) and at most 4 evidence blocks per claim.
   - Citations that do not resolve are skipped; a unit with none resolving is not a claim.
2. **`examples/claim_check.rs`**, run as
   `cargo run -p blyte-agent --example claim_check -- --manifest <m.json> --out <dir> [--model <id>] [--seed-per-report 5] [--max-cost <usd>]`.
   - The manifest is a JSON array of `{ "id": "...", "report": "<path to report.md>", "roots": ["<dir>", ...] }`;
     the first root is the workspace root, the rest are read roots (build `Sources` from them the
     way `check_report_quotes` in `crates/blyte-agent/src/run.rs` does).
   - Build the provider the way `examples/solve.rs` does (same model-id resolution, default the
     dispatch default model). Use `max_output_tokens: None` and no tools.
   - **Seeding:** for each report pick up to `--seed-per-report` claims (deterministically: every
     k-th claim, so reruns pick the same ones) and ask the model, one call per claim, to rewrite the
     claim text so that it is false given the evidence, changing one specific fact (a number, a
     name, a condition, a yes/no), keeping length and style; reply with only the rewritten text.
     Store each as a seeded claim with the same evidence, recording the original id.
   - **Checking:** for each report, send its claims (originals and seeded, shuffled with a fixed
     seed, ids renumbered so seeded ones are not recognisable) in batches of at most 15 per call.
     Prompt: for each claim, using only the evidence shown, answer `supported`, `contradicted`, or
     `not_enough_evidence`, with a one-sentence reason quoting the line number it relies on. Ask for
     a JSON array `[{"id": n, "verdict": "...", "reason": "..."}]`; parse it tolerantly (strip code
     fences), and record a batch that fails to parse as `parse_error` for its claims without
     retrying.
   - **Output** in `--out`: `claims.jsonl` (one line per checked claim: report id, claim id,
     seeded true/false, original id if seeded, text, verdict, reason) and `summary.json` plus a
     printed table, per report and in total: claims, seeded, seeded flagged (`contradicted`),
     seeded flagged or `not_enough_evidence`, originals `contradicted`, originals
     `not_enough_evidence`, parse errors, model calls, input/output tokens, cost (from the
     provider's usage and the same pricing the run report uses), seconds.
   - Stop starting new calls once `--max-cost` (default 10 USD) is reached, and say so in the summary.
3. **Tests** in `test_claims.rs` (no network): a small report string and a temp directory of files
   checks that `claims` finds paragraph and list-item claims, attaches a following code block as
   `quoted`, skips headings, units without citations and citations to missing files, widens and
   numbers the evidence lines and clamps them at file start and end, and applies the 120-line and
   4-block caps. Put any pure helper of the example (batching, JSON reply parsing with code fences)
   in the example file with unit tests there if simple, or skip.

Gate: the repository checks (`blyte.toml`). Commit message:
`feat(research): claim_check example to measure a model claim checker on saved reports`
