# Jev as a Decision Tool in blyte — Exploration Plan

**Status:** planning only (2026-09-27); nothing built. Requested by the user: plan how Jev could be
used inside blyte.

## What Jev is

A decision model from TypeSafe AI (early access since 15 September 2026, version `jev-1.13`).
Instead of writing text, it answers **typed questions about a state**, each with calibrated
probabilities:

- **choice**: one of a fixed set of options (`criteria`: option → description), with a
  probability per option and a `confidence`;
- **score**: a level on an ordered scale of up to 10 described levels;
- **noul**: the probability that a yes/no proposition is true.

Access: `POST https://openrouter.ai/api/alpha/decisions` (or TypeSafe's own API / SDK) with
`{"model": "typesafe/jev-1.13", "state": …, "questions": {…}}`. Price: $0.042 per million input
tokens, output free (a 447-token call costs about $0.00002). Limits: 32K tokens of state, 64K per
request. No published latency; claimed up to 100× faster than an LLM for such decisions.
Recommended for routing, triage, classification, gating agent actions and verifying LLM output;
not for anything that needs prose, or logic a rule can decide.

## Where a calibrated, near-free decision could help blyte

blyte's rule is that every feature earns its place in measurement, and a loop-breaker that
intervened in the loop was built, measured and removed. So the candidates below favour **signals
to the orchestrator** over interventions inside the loop, and each is judged first **offline**,
on runs we already have, before anything is wired in.

| # | decision | where | why it might pay | how to test it offline |
|---|---|---|---|---|
| 1 | **Is this gate failure the environment or the change?** (`noul`, or `choice`: code / environment / flaky) | after a failed gate | the E5 incident (blyte spent 50 minutes clearing a host cache after an environment failure); today the model decides, and the ENV rule only asks it to | label the gate failures in the ledger and eval runs (environment vs code); measure Jev's accuracy and calibration against the labels |
| 2 | **Is the run making progress?** (`score` on the last N tool calls) | `blyte status`, as a field | the `sanitize-git-repo` spiral: 80 of 102 turns spent exploring before the first edit; an orchestrator could kill and re-plan earlier | replay the event logs of past runs turn by turn; check whether low progress scores precede runs that failed or wasted turns, without false alarms on runs that succeeded |
| 3 | **Does the result do what the task asked?** (`score`: task + diff + final report) | `blyte dispatch` summary, as a field for the orchestrator's review | the orchestrator verifies every result; a cheap prior can say where to look hardest | score the 37 paired runs' diffs against their plans; compare with our verdicts (accepted / fixed / redone) |
| 4 | **Delegate or do it yourself?** (`choice`) | the orchestrator skill, before a dispatch | fewer wasted dispatches on tasks that needed a design decision | weaker: needs labelled examples of good and bad delegations we do not have yet |

Recommended order: **1, then 3, then 2.** 1 and 3 have labelled data already (the ledger's gate
failures and verdicts), 1 addresses a real incident, and 3 stays advisory. 2 needs more care so as
not to repeat the loop-breaker. 4 waits for data.

## Steps

1. **Access** (user): a Jev key, directly from TypeSafe (early access) or through OpenRouter,
   which also serves `openrouter/…` models blyte already supports. No key exists in `.env` yet;
   the key is never printed.
2. **Offline study for candidate 1** (Claude, read-only on our data): extract every gate failure
   from the ledger and superstate-eval results with its output, label each (environment, code or
   flaky), send the labelled set to Jev with one `choice` question, and report accuracy,
   calibration (are 0.9-confidence answers right 90% of the time?), cost and latency. Budget: a few
   cents of Jev. Decision gate: wire it in only if it beats a simple rule (for example regexes for
   "No space left", "Could not resolve host", "Connection refused") by a clear margin.
3. **Offline study for candidate 3**, the same way, against the 37 paired runs' verdicts.
4. **If a study passes:** a plan per feature in the usual form (A/B implementation, tests), keeping
   Jev optional: a `jev` section in `blyte.toml` and a key in the environment; without either,
   blyte behaves exactly as today. A Jev call must never block a run: time-boxed, and a failure is
   logged and ignored.
5. **Measure the wired-in feature** on real dispatches before it becomes a default.

## Open questions

- Latency in practice (only "up to 100× faster" is claimed); a gate-failure check must add well
  under a second.
- Whether sending gate output and diffs to a third party is acceptable for the code blyte runs on
  (the same question as for any model provider).
- Early-access stability and rate limits.

Sources: [MindStudio: Jev launch](https://www.mindstudio.ai/blog/jev-system-one-model-launch),
[OpenRouter: What is Jev](https://openrouter.ai/blog/insights/what-is-jev/),
[Pydantic AI: TypeSafe (Jev)](https://pydantic.dev/docs/ai/models/typesafe/),
[LiteLLM: TypeSafe pass-through](https://docs.litellm.ai/docs/pass_through/typesafe).
