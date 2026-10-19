# Spec: check research reports' claims and names, not only their quotes (2026-10-19)

## Why

The quote check (`research_check::check`, 0.32.3; run on early stop since 0.33.1) proves that a
report's quoted code and cited lines match the files. It does not check prose. A session using
blyte for multi-repo research found eight fabrications across four research runs; three
were on 0.33.1 with gemini-4-argon (R3, R4, 2026-10-19). All four invented names were in prose:

| Run | Invented | Caught by the quote check |
|---|---|---|
| R1 | `parent_organization_id`, `credit_pool_owner_id` (never in the code) | no |
| R3 | table `spectra.overwritten_asset_attributes` | no |
| R4 | `MAX_CONCURRENT_SPATIAL_ACTIVITIES = 4` (real: `_MAX_CONCURRENT_CHUNKS = 100`) | partly (line numbers only) |
| R4 | `stale_cache.purge_for_project(...)` (no such function) | partly |

Checked against the logs (2026-10-19):
- R3 to R9 were dispatched with `--research` and no `--allow-read`, while the code to research
  was a pinned copy in the dispatching session's scratchpad. Every `glob`, `grep` and `read` of it
  failed with `path escapes the workspace` (R3: 3, R4: 5); the runs fell back to the main
  checkout, which was months old. blyte gave the dispatcher no warning. `glob`/`grep`/`read`
  already accept absolute paths inside `--allow-read` roots (`resolve_for_read`, `tools.rs`).
- R4's "no such file" problems for `models/assets.py` and `lambda_function.py` were files of other
  repositories the run reached through `bash`, outside every source the check knows. With the
  right `--allow-read` they resolve (the resolver tries each read root and its parent).
- Prose claims are measured (`docs/specs/2026-10-16-research-quote-check.md`, claim checker,
  2026-10-18): 77% recall on seeded errors, ~$0.10 per report, 27/408 parse errors in batches of 15.

## Decisions (user, 2026-10-19)

1. **Claim checker: one fix round.** In a research run's gate, after the quote check passes, a
   model checks each claim (`research_check::claims`) against its cited lines. Contradicted claims
   go back to the model once to fix or remove. The next gate pass checks again, and whatever is
   still contradicted is recorded and shown in `blyte review`; it does not fail the gate.
   `not_enough_evidence` is counted and shown, never fed back.
2. **Checker model: the run's own model** (provider and model of the run), as measured.
3. **Batching** (from the measurement): at most 8 claims per call, one retry of a batch whose reply
   does not parse. At most 64 claims per report (the rest are counted as unchecked). The checker's
   usage counts toward the run's usage and `--max-cost`.
4. **Also on early stop.** A research run that stops without a passing gate gets the claim check
   too (advisory, as the quote check on stop).
5. **On by default for `blyte dispatch --research`**, off by default in the library `Runner` (so
   scripted-provider tests are unaffected); `--no-claim-check` turns it off for a dispatch.

Added from the fabrication report, deterministic and cheap (Claude's recommendation, 2026-10-19):

6. **Name check, part of the quote check (blocks like it).** Every code-like name in a backticked
   span of report prose must appear in something the run saw: the task, or any tool result. A name
   is code-like when it contains `_`, a `.` between identifiers, an inner capital (`camelCase`), or
   is followed by `(`. All four invented names above fail this; real names pass because the run
   read them. The set of seen identifiers is kept for the whole run, so folding does not lose it.
7. **Unreadable paths are loud.**
   - `blyte dispatch --research` refuses to start when the task text names an absolute path that
     exists but lies outside the workspace and every read root, and says which `--allow-read` to
     add; `--no-path-check` overrides. Other dispatches print a warning.
   - The `path escapes the workspace` error a read tool returns says what the run can read and
     tells the model to report the path as unreadable rather than read another copy.
   - `blyte review` lists refused reads under `== checks` (count and first 3 paths).

## Not done

- Marking unchecked report sections near the time limit: the quote, name and claim checks on early
  stop already flag what is wrong.
- Confining research `bash` reads to the read roots: a separate question (sandbox).
- A cheaper fixed checker model: unmeasured.

## Data after the `--allow-read` fix (repos session, 2026-10-19)

Redispatched with the right `--allow-read` (R5b–R9b: 0 refused reads), the runs still named
identifiers found nowhere in the three pinned trees (`grep -rF`): 1/92, 6/105, 14/116, ~16/100,
~20/91. Most are near-misses (`asset_hazard_run_summaries` for `asset_hazard_run_summary`,
`calculate_rp_Losses` for `add_rp_based_losses`, invented `FETCH_*` constants). So the name check is
needed even when every path is readable. Follow-up, not built: label names seen only in an input
file under review (a draft the run was asked to correct) as coming from the input, rather than
letting them pass silently.
