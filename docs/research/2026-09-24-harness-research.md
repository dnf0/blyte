# Harness research and measurements, 2026-09-24

What we measured on our own harness, what current external research says, and the objective that follows. Evidence lives in superstate-eval FINDINGS 13.39–13.44. This note is the synthesis.

## Objective

**Make the harness as computationally efficient as possible**, in both of the ways it spends compute:
1. **The model's compute:** turns × prompt size, plus output. This is the bill: about 90% of cost and wall time.
2. **The harness's own compute:** process spawning, tool execution, context building. This is the rest.

A feature earns its place only if it improves one of these, or completion, in repeated measurement. Otherwise it is removed.

## What we measured

| Finding | Evidence |
|---|---|
| Superstate beats agy on cost and time at an equal pass rate. On build-cython-ext its worst run was cheaper and faster than agy's best: $1.32–1.43 and 539–592s for `folded`, against $1.45–1.70 and 816–902s for agy. | repeats + agy repeats, 2026-09-24 |
| agy writes 2–4× the output tokens (planning and narration), and output is the expensive, slow token. Its extra ~38K tokens alone come to roughly 150–250s. | 13.39, 13.41 |
| **Turn count drives cost.** Cost per turn is steady within a configuration (sd about 5%). Runs that cost more are runs that took more turns. | 13.43, 13.44 |
| About 94% of fresh input is Gemini implicit-cache misses. The cache starts only when the prompt passes about 16K tokens, matches in blocks of about 4K, and lags a turn or two. That is about $0.0033 per turn of fixed overhead. Explicit caching would not pay (creation bills the prefix at the fresh rate). | 13.43 |
| One turn costs as much as about 100–200K cached tokens. So compressing the prompt pays only if it costs almost no extra turns. | 13.43, 13.44 |
| Context configuration did not move pass rate. The only failures were the same missed requirement, in three different configurations: a comprehension or completeness miss, not a context effect. | 13.44 |
| `folded` (48K, mechanical digests, prompt only grows between folds) is the best default. `compact` gained nothing. A 16K budget was −20% cost on build-cython but +77% turns on scc. Fold notes (LLM summaries) did not pay. | 13.41–13.44 |
| The model makes one tool call per turn, whatever the prompt says. | 13.37 |
| **The harness's own overhead:** each `docker exec` costs 60–110 ms, and a login shell (`bash -lc`) adds about 35 ms. With 2–3 execs per turn, that is about 40–55s of a ~570s run (8–10% of wall time). | measured 2026-09-24 |

## What external research says (September 2026)

1. **The harness changes cost far more than pass rate.**
   - Three harnesses on one model spanned up to 40× in tokens per solved task, with pass-rate differences of 0–8 points (Vats & Golev, *The Scaffold Effect*, 2607.22585).
   - Holding the model fixed and changing only the harness moves leaderboard scores 9–48 points.
   - This matches our data.
2. **Context management matters for tight windows, little for wide ones.** Managed context beat none by 35.7 points at 32K but only 2.7 at 128K (*An Empirical Study of Harness Design for Coding Agents*, 2609.20804). Gemini's 1M window puts us in the wide regime: context management buys cost, not correctness.
3. **Cheap mechanical trimming beats clever machinery.** Rule-based elision gave the best efficiency, with LLM summarisation as a later backstop. A tool for recovering elided content went unused in 56% of settings and added no accuracy (2609.20804). This backs `folded`, explains why fold notes did not pay, and says not to build recall.
4. **Coarser action spaces can be cheaper for strong models.** Bash-only was 53% cheaper and 3.6 points more accurate for a strong model on SWE-bench, with 20–30% fewer tool calls from composite commands. Weak models and file-heavy tasks still benefit from predefined read/edit tools (2609.20804).
5. **A light plan cuts cost for strong models:** about −30% cost and −24% turns, by skipping redundant verification. It helps weak models' accuracy instead (2609.20804).
6. **The field is moving to adaptive or learned harnesses** (CHILL-Harness 2607.25825, openJiuwen 2608.27969, Natural-Language Agent Harnesses 2603.25723), and to always disclosing the harness in comparisons (2605.23950).

## Where the remaining efficiency is

- **Model compute (the big one):** fewer turns. The research-backed levers not yet tried are a coarser action space and a light upfront plan. The adaptive fold budget is being measured.
- **Harness compute (8–10% of wall time):**
  - do file tools natively in Rust instead of through `docker exec`;
  - keep one persistent shell per run instead of spawning a login shell per command;
  - drop redundant probes (for example the per-grep `rg` availability check).

## Sources

- [An Empirical Study of Harness Design for Coding Agents](https://arxiv.org/html/2609.20804v1)
- [The Scaffold Effect in Coding Agents](https://arxiv.org/pdf/2607.22585)
- [Same Model, Different Harness: Different Coding-Agent Results](https://arxiv.org/pdf/2608.26218)
- [Harness or Model? Isolating the Harness Effect in Agentic Coding](https://arxiv.org/pdf/2609.11987)
- [SWE-bench in 2026: Benchmarks vs Scaffolding Reality](https://www.digitalapplied.com/blog/swe-bench-verified-june-2026-benchmark-vs-scaffolding-analysis)
- [Stop Comparing LLM Agents Without Disclosing the Harness](https://arxiv.org/pdf/2605.23950)
- [CHILL-Harness](https://arxiv.org/pdf/2607.25825)
- [openJiuwen: Beyond Static Harnesses](https://arxiv.org/pdf/2608.27969)
- [Natural-Language Agent Harnesses](https://arxiv.org/pdf/2603.25723)
- [Context as an Environment](https://arxiv.org/pdf/2608.21690)
- [Gemini Developer API pricing](https://ai.google.dev/gemini-api/docs/pricing)
