# Reading note: Harness-Zero (arXiv 2609.24974)

**Paper:** Ye, Lu, Dong, Su and Song, "Harness-Zero: Harness Distillation via Agent-as-Harness",
21 September 2026. Read in full on 2026-09-28.

**Verdict: not useful for blyte in general.** Noted for the idea log only.

## What it does

Agents do better inside a specialised harness (tools, middleware rules, skills, memory), but a
general agent cannot carry one per domain. Harness-Zero moves the harness's behaviour into the
model's weights instead:

- A **harnessing agent** (GPT-5.6 Sol, closed) reviews each proposed action of a **student**
  (Qwen3.5-9B, open weights) against a reference harness, and corrects it before it runs in a
  minimal harness (one Bash tool). Only accepted, corrected actions become training data.
- The student is fine-tuned on those trajectories (LoRA SFT, two epochs, a few hundred rollouts per
  domain).
- Domains: SpreadsheetBench, AppWorld and USPTO retrosynthesis. **No coding-agent benchmark.**

## Results

- The distilled 9B model, **with no harness**, averaged 44.3% against 23.3% for the base model and
  41.7% for the base model with the full harness attached. It recovered 82% of 28 harness-induced
  behaviours: inspecting before editing, reading back what it wrote, validating the final answer,
  not repeating a failed call.
- Without training, a frontier model reviewing a frontier model's actions ("agent as harness")
  averaged 81.1%, against 78.1% for the evolved code harness and 68.6% for a minimal one.
- Limits they state: a weaker harnessing agent's reviews are harmful; deep domain knowledge
  transfers less well than procedure (USPTO: 30% distilled against 38% with the harness); the
  review step made collection 2.4 times slower; no compute or cost figures.

## Why it does not apply

- **It needs trainable weights.** blyte runs Gemini and Claude through their APIs.
- **The behaviours it transfers are ones blyte already gets from its design**: the sealed gate
  checks the result, whole-file reads and full failure reports make inspection cheap.
- **No coding tasks, no cost measurements, and nothing on tool-call batching**, which is the
  Gemini behaviour we could not change by prompting (one call per turn).

## Where it touches our results

- "A weaker harnessing agent's reviews are harmful" matches offload v2: a plan written by blyte on
  Gemini was worse than no plan, while Claude's plan made blyte pass cheaply (blyte-eval
  `results/offload-v3-blyte-20260928-1754/`).
- If blyte ever runs an open-weight implementer, distilling behaviours prompting cannot reach (such
  as batching tool calls) is the idea to revisit. That would be a research project, not a harness
  change.
