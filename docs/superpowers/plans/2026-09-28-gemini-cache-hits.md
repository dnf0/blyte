# Gemini Cache Hits — Plan

**Status:** planned 2026-09-28; build after the offload v2 runs finish (changing blyte mid-experiment
would break the comparison).

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

(A miss here: fresh input above both 4K tokens and three times the new content.) About 1.1M tokens
were billed at $0.75/M instead of $0.075/M: roughly $0.75, 30% of the run's $2.56 of Gemini.

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

## Step 2: explicit context caching (only if Step 1 says so)

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
