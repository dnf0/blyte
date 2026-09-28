# Gemini Cache Hits — Plan

> **Correction (2026-09-28, later the same day):** the miss counts below overstate the problem. Gemini
> caches in 4,096-token blocks and serves at best the previous prompt rounded down to a block, so up
> to 4K of fresh input a turn is structural, and the first rule counted it as a miss. Scored
> block-aware (a true miss serves at least one block less than that), blyte's true misses are
> **22% of turns in B1 (0.38M tokens, about $0.26, 10% of its Gemini cost)** and **24% over 5,573
> earlier turns (10.2M tokens, about $7)**. A 16-turn agy probe on Vertex AI had none (about 4
> expected at blyte's rate, p ≈ 0.01). Streaming (`streamGenerateContent`) made no difference
> (a 56-turn probe, 43% vs 46% by the old rule, identical by block). The likely fix is the Vertex AI
> endpoint, which needs Google Cloud credentials; fewer turns remains the larger saving.

**Status:** closed 2026-09-28, nothing built. Step 1 showed the misses are Gemini's; a probe
(below) showed that explicit caching would cost more than it saves for blyte's growing context.

## The finding

In offload v2 run B1 (superstate-eval `results/offload-v2-sonnet-20260928-1028/ab-1`), blyte's three
dispatches on Gemini 3.8 Flash took 237 turns. The folded context is append-only between folds, so
on an appended turn only the new messages should be billed as fresh input. They were not:

| dispatch | turns | fresh input on appended turns | new content on those turns (chars / 4) | turns with a cache miss |
|---|---|---|---|---|
| 1 | 110 | 667K | ~104K | 53 |
| 2 | 43 | 235K | ~52K | 16 |
| 3 | 84 | 427K | ~85K | 46 |
| total | 237 | 1.33M | ~241K | 115 of 229 |

(A miss here: fresh input above both 4K tokens and three times the new content.) Counting only the
missed turns, 0.74M tokens beyond their new content were billed at $0.75/M instead of $0.075/M:
about $0.50, 20% of the run's $2.56 of Gemini ($0.74, 29%, if every appended turn had billed only
its new content).

The cost arithmetic itself is right: the adapter subtracts `cachedContentTokenCount` from
`promptTokenCount` before pricing (`gemini.rs`). The request body is built deterministically and
only grows at the end between folds (`GeminiProvider::body`). So the likely cause is that Gemini's
implicit caching is best-effort (no hit is guaranteed for an identical prefix), but that is not yet
shown.

## Step 1: diagnose (no build; a few cents)

Establish whether the misses are ours or Gemini's before building anything.

1. **Byte identity.** Add a debug-only record per turn (behind `BLYTE_DEBUG_PREFIX=1`, not a
   feature): the SHA-256 of the serialised request body with the newest messages removed, compared
   with the previous turn's full body. Run one small dispatch (a synthetic 30-turn task on a
   throwaway repo, about $0.05). If any appended turn's prefix differs from the previous body, the
   misses are ours: fix the rendering and stop here.
2. **Correlates**, from the B1 reports (offline, free): do misses follow long gaps between turns
   (the gate, a slow command), parallel tool-call batches, or turns just after a fold? A strong
   time correlation points at cache expiry and suggests a cheaper fix than explicit caching.
3. **Baseline hit rate** on the FrontierHarness Gemini runs already in superstate-eval `results/`,
   to see whether B1 is typical (use the same miss rule).

Decision: our bug → fix it (a unit test pins prefix stability). Gemini's → Step 2.

### Step 1 findings (offline, 2026-09-28)

Items 2 and 3, on B1 and on 88 earlier blyte-on-Gemini reports in superstate-eval `results/`
(5,571 appended turns):

| | B1 | earlier runs |
|---|---|---|
| miss rate | 50% | 53% |
| excess fresh input on missed turns | 0.74M | 20.0M (about $13.50) |

- **Not expiry:** turns less than 5 s after the previous request miss 51% of the time.
- **Not size:** about 50% at every prompt size (under 32K, 32–64K, over 64K).
- **Not folds:** the first turn after a fold misses less (36%).
- **Clustered:** a miss follows a miss 69% of the time, a hit 35%. That fits requests being served
  by backends that do not hold the cache, i.e. implicit caching being best-effort.

So the misses look like Gemini's, and Step 2 is the fix; item 1 (byte identity) remains as a quick
confirmation with the first Step 2 task.

**Also found:** on 5,565 of those 5,571 turns, Gemini made a single tool call, though blyte's system
prompt asks for independent calls to be batched in one turn. Each avoided turn saves re-sending the
whole context (about 58K tokens in B1), so getting Gemini to batch may be worth more than the cache
fix. That is its own plan.

## Step 2: explicit context caching — rejected after a probe

**Probe (2026-09-28, about $0.10):** a 28,097-token explicit cache (system, tools, contents), then five
turns each appending about 6K tokens. `cachedContentTokenCount` stayed at exactly 28,097 on every
turn while `promptTokenCount` grew 34,276 → 59,076: the suffix after an explicit cache is billed
fresh every turn, and implicit caching does not cover it. Creation is metered on the full cache
(`usageMetadata.totalTokenCount` 28,097) plus storage ($0.50/M tokens/hour). Caches are immutable.

So for a context that grows each turn, a cache must be recreated to cover new turns. At B1's rates
(about 1K new tokens a turn, 60K context), recreating every R turns costs about R/2 K fresh tokens
a turn for the tail plus 60K/R at creation: best near R = 11, about 11K fresh-equivalent a turn,
against about 4K today (1K new plus half of the turns missing about 6.4K). Implicit caching, even
missing half the time, is cheaper, because its misses are partial. Not built.

The original design, kept for the record:

Gemini's `cachedContents` API stores a prefix (system instruction, tools, contents) and returns a
name; `generateContent` then sends `cachedContent: <name>` plus only the contents after it, and the
cached part is billed at the cached rate with a guaranteed hit. Caches are immutable and carry a
TTL and an hourly storage charge.

Design, to be confirmed against the current documentation before any code (prices, the minimum
cacheable size for Flash, the storage rate, whether creating a cache bills its tokens once at the
input rate, and rate limits on cache creation):

- The provider keeps one live cache per run. It creates it once the stable prefix passes the
  minimum size, and recreates it (then deletes the old one) when the uncached tail passes a
  threshold (start at 32K tokens; the value is measured, not guessed) or after a fold rewrites
  the prefix. Folds are rare (2–3 per dispatch in B1).
- TTL short (start at 10 minutes, refreshed on use); the cache is deleted when the run ends,
  including on error and budget exhaustion.
- Opt-in first (`--gemini-cache=explicit`); the default changes only after measurement.
- The report records per run: caches created, cache storage cost, hit rate, and fresh versus
  expected fresh input, so the saving is measured net of storage and creation.

Tasks (A/B-dispatched as usual, one plan task each): the provider support and its wire tests
(mocked HTTP: create, use, recreate on threshold and fold, delete on drop); the report fields;
the flag.

## Step 3: measure

Repeat the B arm of offload v2 (same plan, same task) with the fix, 2 runs, and compare against
B1/B2: Gemini cost, fresh input against expected, hit rate, wall time. Keep it only if Gemini cost
falls by more than the spread between B1 and B2. The same measurement on one or two FrontierHarness
tasks checks that it helps beyond this one task.

## Also worth checking (separate from caching)

The same reports show the other half of B1's Gemini bill: 237 turns re-sending a context that
averaged about 58K tokens. Fewer turns saves more than any cache fix. The dispatch event logs added
in superstate-eval #73 show how often Gemini batches independent calls into one turn, and how much of
its command time went on full test-suite runs. That is its own plan once B2's logs are in.
