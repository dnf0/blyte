# Spec: stream Gemini responses with an idle timeout

## Problem

Argon can think for minutes before it answers, so a Gemini call either has a whole-call timeout
long enough for the slowest live call (`BLYTE_MODEL_TIMEOUT_SECS=600` on this machine) or cuts
live calls off. A dead connection then also takes up to 600 s to notice.

## Facts (checked 2026-10-15)

- `Gemini::complete` posts to `:generateContent` and reads the whole JSON body
  (`crates/blyte-provider/src/gemini.rs`, around line 565).
- The shared `http_client()` (`crates/blyte-provider/src/lib.rs`) sets `.timeout(whole)` and
  `.timeout_read(whole - 30)`. In ureq 2.12 a whole-call timeout sets every socket read's timeout to
  the time left before that deadline (`ureq-2.12.1/src/stream.rs` lines 86-88 and 434), and the
  separate read timeout is not used. Today there is only the whole-call timeout.
- A probe of `gemini-4-argon:streamGenerateContent?alt=sse` with `thinkingLevel: high` and a prompt
  that made it think:
  - thought summaries off (`includeThoughts` false, blyte's request today): nothing for 64 s,
    then the answer in chunks 0.1-0.4 s apart;
  - thought summaries on: the first chunk after 3.4 s, a thought-summary chunk every 3-6 s while
    it thinks, longest gap 6.2 s.
- Streaming is only worth doing with thought summaries on.
- Cost of thought summaries (decision 4), measured on 4 prompts with `thinkingLevel: high`, each
  run with summaries on and off:
  - off: 2186 thinking + 8084 output tokens; on: 2282 thinking + 8003 output tokens;
  - $0.103 either way, so summaries cost nothing measurable;
  - longest gap between chunks: 11.4 s with summaries on, 24.2 s with them off.
- blyte already ignores thought parts in a response, except for their `thoughtSignature`.

## Decisions (user, 2026-10-15)

1. Gemini calls stream (`:streamGenerateContent?alt=sse`) on both the API-key and Vertex routes,
   with `generationConfig.thinkingConfig.includeThoughts: true`.
   - The summaries are discarded as thought parts are today; none are shown.
   - Setting `BLYTE_GEMINI_THOUGHT_SUMMARIES=0` turns them off, which also means long silent gaps
     again.
2. Timeouts for streamed calls:
   - Idle: `BLYTE_MODEL_IDLE_TIMEOUT_SECS`, default 90, about 15 times the longest gap seen. The
     call fails when no bytes arrive for that long.
   - Cap: `BLYTE_MODEL_TIMEOUT_SECS` becomes the whole-call cap for streamed calls, default 1800,
     so a call that trickles forever still ends.
   - The cap is checked by blyte between chunks, since ureq can't combine the two timeouts, so it
     may run over by up to the idle timeout.
3. Other providers don't stream and keep today's client and whole-call timeout (default 180 s)
   unchanged.
4. Before merging, measure the cost of thought summaries:
   - several paired calls, summaries on and off, on the same requests;
   - compare thinking and output tokens.
   - If summaries cost noticeably more, come back with the numbers before merging.

## Design

- **A second client.** `streaming_http_client()` in `lib.rs` has a connect timeout and
  `timeout_read(idle)`, and no `.timeout(..)`. Only Gemini uses it.
- **Merging the stream.** Gemini reads `data:` lines from the response and merges the chunks into
  one response with the same shape `generateContent` returns:
  - parts are appended in order;
  - consecutive text parts with the same `thought` flag and no `functionCall` are concatenated
    with no separator, keeping the latest `thoughtSignature`;
  - the latest `usageMetadata`, `promptFeedback` and `finishReason` are kept.

  The existing `Gemini::parse` then reads the merged value, so no-answer handling, call ids,
  signatures and usage keep their current behaviour.
- **Errors.**
  - An I/O error or idle timeout mid-stream becomes `ProviderError::Transport`. That is already
    transient and retried, and a retry starts over and keeps nothing from the failed attempt.
  - Going over the cap becomes `Transport` with the text "timed out", so the agent's existing
    timeout message and retry apply.
  - A JSON error object in a `data:` line, or a non-200 status, becomes the same `Api` error as
    today.
- **Messages and docs.** The agent's timeout message names both variables. `docs/providers.md`
  documents them.

## Out of scope

- Showing thought summaries.
- Streaming for other providers.
- Acting on partial output before the stream ends.
