# Plan: stream Gemini responses with an idle timeout

Spec: `docs/specs/2026-10-15-gemini-streaming.md`. One task, since every part depends on the
same new client and parser.

### Task GS1

**Goal:**
- Gemini calls stream with thought summaries on.
- They fail after `BLYTE_MODEL_IDLE_TIMEOUT_SECS` (default 90) with no bytes, or once
  `BLYTE_MODEL_TIMEOUT_SECS` (default 1800 for streamed calls) has passed.
- Other providers are unchanged.

**Where:**
- `crates/blyte-provider/src/lib.rs`:
  - `model_timeouts_from` / `model_timeouts` / `model_timeout_secs` (around lines 30-55);
  - `build_http_client` and `http_client()` (around lines 57-76).
- `crates/blyte-provider/src/gemini.rs`:
  - `vertex_url` (around line 45, builds `...:generateContent`);
  - `Gemini::endpoint_url` (around line 300);
  - `Gemini::body` (the `generationConfig`/`thinkingConfig` block, around lines 400-409);
  - `Gemini::parse` (starts about line 415);
  - `Gemini::complete` (around line 565: the API-key and Vertex requests, the 401 retry, and
    how the response and HTTP errors are read).
  - `list_models` checks `supportedGenerationMethods` for `generateContent` (around line 522);
    leave that as it is.
- `crates/blyte-provider/src/chatgpt.rs` has an SSE parser (`parse_sse`, line 253); copy its style
  where it helps.
- `crates/blyte-agent/src/run.rs`: the "model call timed out after {s} s; retrying" message
  (around line 754).
- `docs/providers.md`: where `BLYTE_MODEL_TIMEOUT_SECS` is documented.
- Tests:
  - `crates/blyte-provider/tests/test_gemini_wire.rs` (body and parse);
  - `test_gemini_vertex.rs`, which asserts `:generateContent` URLs (around lines 657-661 in
    `gemini.rs` tests and in this file); update them;
  - `test_model_timeouts.rs`;
  - `crates/blyte-agent/tests/test_model_timeout_transport.rs`, which has a stalling
    `TcpListener` to copy.

**Change:**
1. `lib.rs`:
   - Add `model_idle_timeout_secs()`: `BLYTE_MODEL_IDLE_TIMEOUT_SECS`, default 90, read once.
     Invalid or zero values fall back to the default.
   - Add `model_stream_cap_secs()`: `BLYTE_MODEL_TIMEOUT_SECS` when set, else 1800.
   - Add `build_streaming_http_client(idle)`: a 20 s connect timeout and `timeout_read(idle)`,
     with no `.timeout(..)`. Add a comment saying why: in ureq 2 a whole-call timeout replaces
     the read timeout.
   - Add `streaming_http_client()`, cached like `http_client()`.
   - Keep `http_client()` and the 180 s default exactly as they are for other providers. Make the
     pure helpers testable without the environment, as `model_timeouts_from` is.
2. `gemini.rs`:
   - Both URLs use `:streamGenerateContent?alt=sse`.
   - `complete` uses `streaming_http_client()` and sets `Accept: text/event-stream`.
   - `body` sets `thinkingConfig.includeThoughts: true` unless `BLYTE_GEMINI_THOUGHT_SUMMARIES`
     is `0`, alongside `thinkingLevel` when that is set. Keep `body` pure: pass the setting in or
     read it in the constructor, so tests can set it.
3. Add `Gemini::parse_stream<R: BufRead>(reader, cap: Duration, started: Instant) ->
   Result<Response, ProviderError>`:
   - Read lines, take `data:` payloads, and parse each as JSON.
   - Merge the chunks into one value shaped like a `generateContent` response:
     - candidate parts are appended in order;
     - a text part with no `functionCall` is concatenated onto the previous part when that is
       also text with the same `thought` flag (no separator), and a later `thoughtSignature`
       replaces the earlier one;
     - keep the latest `usageMetadata`, `promptFeedback`, `finishReason` and `modelVersion`.
   - Then return `Self::parse(&merged)`.
   - A `data:` payload that is an error object (`{"error": {...}}`) returns the same error type
     `complete` gives an HTTP error body today.
   - An I/O error, including the idle timeout, returns `ProviderError::Transport` with the error
     text. Keep "timed out" in it when it was a timeout.
   - After each line, if `started.elapsed() > cap`, return `Transport("timed out: stream exceeded
     {cap}s")`.
   - A stream that ends with no chunks returns `NoAnswer` with no reason, as an empty response
     does today.
4. `run.rs`: the timeout retry message names `BLYTE_MODEL_IDLE_TIMEOUT_SECS` (silence) and
   `BLYTE_MODEL_TIMEOUT_SECS` (whole call) as the settings to raise. Keep it one line.
5. `docs/providers.md` documents:
   - streaming for Gemini;
   - the idle timeout and the 1800 s cap for Gemini;
   - the 180 s whole-call timeout for the other providers;
   - `BLYTE_GEMINI_THOUGHT_SUMMARIES=0`.

**Tests** (no network):
- `parse_stream` with canned SSE bytes (`std::io::Cursor`):
  - text split over three chunks joins with no newlines;
  - thought chunks then a text chunk carrying `thoughtSignature`: the text and signature come
    through, and the thought text doesn't;
  - a `functionCall` in a later chunk: its args and signature come through, and call ids are
    `gemini_call_<n>` as `parse` numbers them;
  - `usageMetadata` only on the last chunk;
  - a stream with only `promptFeedback.blockReason` gives `NoAnswer` with that reason;
  - an error object in a `data:` line;
  - an empty stream;
  - a cap of 0 s with `started` in the past gives a timed-out `Transport` error.
- `body` includes `includeThoughts: true` by default, and leaves it out when summaries are
  turned off.
- The idle timeout over a socket: a local `TcpListener` server (as in
  `test_model_timeout_transport.rs`) that sends HTTP 200 headers and one SSE chunk, then stalls.
  A client from `build_streaming_http_client(1 s)` reading through `parse_stream` fails with a
  `Transport` error within a few seconds.
- A server that sends a chunk every 0.5 s for 3 s and then ends succeeds with the same 1 s idle
  timeout.
- The URL tests assert `:streamGenerateContent?alt=sse`.
- The timeout helpers' defaults and parsing.
