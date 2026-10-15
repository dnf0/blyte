# Plan: Gemini empty responses, and the model in dispatch summaries

In the Argon benchmark (2026-10-14/15) 7 runs ended at once with
`could not parse provider response: no candidates in {"promptFeedback":{"blockReason":"SAFETY",...}}`.
Gemini returned no candidates: in at least 4 of them its safety filter had blocked the prompt. The same tasks
passed on other attempts, so one retry would often have saved the run, and when it doesn't, the
error should say what happened rather than look like a parse bug. Separately, a dispatch summary
(`.wt/<id>.json`) doesn't say which model ran; only the full report does.

GE1 and DS1 touch different files and are independent.

### Task GE1

**Goal:** a Gemini response with no candidates is a named, retried-once error, not a parse error.

**Where:**
- `crates/blyte-provider/src/gemini.rs`, `parse` (around line 416): today a missing
  `candidates[0]` is `ProviderError::Parse(format!("no candidates in {v}"))`.
- `ProviderError` in `crates/blyte-provider/src/lib.rs` (around line 91).
- `is_transient` in `crates/blyte-agent/src/run.rs` (line 223) and the retry loop around line 720.

**Change:**
1. Add a `ProviderError` variant for "the model returned no answer", carrying the block reason
   and message from `promptFeedback` when present (`blockReason`, `blockReasonMessage`), or
   `None` when absent. Its display text says plainly what happened, for example
   `Gemini returned no answer: prompt blocked (SAFETY): The prompt is blocked due to safety`,
   and does not dump the whole response body.
2. `parse` returns it when there are no candidates. Other parse failures stay `Parse`.
3. The run loop retries this error once, and only once, whatever `MAX_PROVIDER_RETRIES` is: the
   conversation is the same on the retry, so repeated retries of a deterministic block only
   waste time. Use the existing retry wait. After the one retry, the run ends with the error as
   today (`Outcome::Error`), now with the clear message. Count the retry in
   `report.provider_retries`.
4. If the empty response carries `usageMetadata`, those tokens are still added to the run's usage,
   so blocked calls are not free in the report. If that doesn't fit the error path cleanly, skip
   it and say so in the summary.

**Tests:**
- `parse` on `{"promptFeedback":{"blockReason":"SAFETY","blockReasonMessage":"..."}}` gives the
  new variant with both fields; on `{}` it gives the variant with `None`; a malformed candidate
  still gives `Parse`.
- With a scripted provider that returns the new error once and then a normal response, the run
  continues and `provider_retries == 1`. When it returns the error twice, the run ends with an
  error whose text contains "no answer", after exactly one retry.

### Task DS1

**Goal:** the dispatch summary names the model that ran.

**Where:** `crates/blyte-cli/src/dispatch.rs`: the summary struct (it has `cost_usd` at around
line 108) and where it is filled from the run report (around lines 671 and 707). The run report
(`RunRecord` in `crates/blyte-cli/src/report.rs`) already has `model_id`.

**Change:** add `model: Option<String>` to the summary, filled from the report's `model_id`,
placed next to `cost_usd`. Add it wherever the summary is built, including the paths that build
it without a report (`None` there). Update any docs that list the summary's fields (search
`docs/` for `gate_attempts` or `cost_usd` in a field list).

**Tests:** a dispatch whose report has `model_id: "gemini/gemini-4-argon"` gives a summary with
`model == "gemini/gemini-4-argon"`; the existing summary tests still pass.
