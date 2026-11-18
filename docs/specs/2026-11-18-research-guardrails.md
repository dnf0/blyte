# Research runs: guidance and guardrails

**Date:** 2026-11-18

## Problem

blyte's research runs work best on narrow questions, but nothing in blyte says so or checks it.
Measured on 19 research runs on 2026-11-18:

- model calls are 90% or more of the wall-clock time in 15 of 19 runs; running tools is under 3%;
- in run R-DA, 1,587 s of 1,659 s of timed model calls passed before the first byte came back, and
  calls with prompts over 100K tokens waited 31 s on average against 7 s under 100K;
- three broad audit questions each used the full 30 minutes (USD 3.30 to 5.60); two ended
  `budget_exhausted` and one report was unusable. Narrow questions finished in 3 to 10 minutes for
  USD 0.20 to 0.50.

Published practice agrees (Anthropic's multi-agent research write-up, Cognition's "Don't Build
Multi-Agents", LangChain's summary, Chroma's context-rot study): read-only work splits well into
narrow runs with fresh contexts; each needs an explicit brief; long prompts lower quality; one
coordinator combines short, cited results.

## Decisions

blyte gives guidance and applies deterministic guardrails. No extra model calls, no planner.

1. **A brief is required.** A research question must have four headed sections: `Objective`,
   `Start from`, `Out of scope`, `Answer format`. `blyte dispatch --research` stops with exit 2 and
   names each missing section. `--no-brief-check` starts anyway.
2. **A template.** `blyte dispatch <id> --research` with no question prints the template with the
   error, so the orchestrator can fill it in.
3. **Unverified reports are not inputs.** When the question names a Markdown file that starts with
   blyte's `> **Not verified.**` line, dispatch stops with exit 2 and names the file.
   `--no-brief-check` starts anyway (for example to ask a run to check that report).
4. **A breadth flag in review.** `blyte review` flags a research run in which 10 or more turns had a
   prompt over 100,000 tokens (fresh plus cached input).
5. **A split suggestion in review.** When a research run ends `budget_exhausted`, review lists the
   directories it read files from, with the number of distinct files in each.
6. **Guidance in the bundled skill and the usage guide:** ask one subsystem or page per run (about
   ten files or fewer), in parallel; a second round takes verified first-round reports as inputs and
   asks only what crosses boundaries; the orchestrator combines the results.

## Not doing

- Judging a question's breadth before the run (counting paths, word lists): noisy; the breadth flag
  measures the real thing.
- A research planner or automatic splitting in blyte: splitting needs a map of the code, which is the
  orchestrator's job.
- Changing the 180K research fold budget or the 30-minute default: small questions stay under both.

## No fallbacks

Each check stops with an error that names what is missing. A flag in review is a prompt to the
reviewer, as the fallback flag is, and never changes the run's outcome.
