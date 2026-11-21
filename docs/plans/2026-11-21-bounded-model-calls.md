# Plan: a model call always returns within a bounded time

On 2026-11-20 a `blyte --orchestrate` session on Gemini blocked for 34 minutes, and then again
for 15, inside one model call (a process sample showed `ureq`'s `DeadlineStream` in
`read_next_line`, that is, waiting for the response's status line and headers). No timeout
fired. A research run on 2026-11-21 (`RS1`) traced it to the streaming HTTP client in
`crates/blyte-provider/src/lib.rs`, which sets only a read timeout and no whole-call timeout:

- ureq 2.12.1 clears a socket's timeouts when it returns the connection to its pool, and does
  not set the read timeout again on a reused connection until the response headers have been
  read. Waiting for headers on a reused connection therefore has no timeout at all.
- The read timeout is per socket read, so a server that sends a byte now and then, without a
  newline, never trips it, and the stream cap is only checked after a whole line arrives.
- Neither name resolution nor writing the request has a timeout.

These points come from reading the code; the first test below is what confirms the first one.

### Task A1: bound every Gemini and Vertex model call

**Depends on:** none. **Siblings:** tasks S1, S2 and S3 of another plan run at the same time in
other worktrees, in `crates/blyte-agent` and `crates/blyte-cli`; none of their code is needed.

**Goal.** `Gemini::complete` (API-key and Vertex) returns `Err(ProviderError::Transport(..))`
for which `is_timeout()` is true:
- within the idle limit when no response bytes arrive (`model_idle_timeout_secs()`, or
  `req.timeout` when that is shorter), at any stage: connecting, sending the request, waiting
  for the status line and headers, or between pieces of the body;
- within the whole-call cap (`model_stream_cap_secs()`, or `req.timeout` when that is shorter)
  however the bytes arrive.
"Within" allows 2 seconds of slack. This must hold on a reused pooled connection too.

**Files.** `crates/blyte-provider/src/lib.rs`, `crates/blyte-provider/src/gemini.rs`,
`crates/blyte-provider/tests/test_request_timeout.rs` (add tests; existing tests pass
unmodified). No other file. No new dependency.

**What to build.**

1. The request and the reading of its response run on a helper thread that sends what it reads
   (the response head, then the body in pieces as they arrive, then end or error) over an
   `std::sync::mpsc` channel. The calling thread takes them with `recv_timeout` of
   `min(idle, time left before the cap)`. When that times out the call returns the timeout
   error at once; the helper thread is left to finish on its own and its result is dropped.
   Follow the pattern already used for subprocess output in this crate (`output_with_timeout`
   in `gemini.rs`).
2. So that an abandoned helper thread cannot live for ever, the streaming agent also gets
   ureq's whole-call `.timeout(cap + 5 s)` and a `.timeout_write`. An abandoned thread must end
   within the cap plus 10 seconds.
3. The existing parsing of the stream (`read_stream` / `parse_stream_timed`) keeps its
   behaviour for well-formed input: same merged response, same errors for malformed streams.
4. Vertex token refresh (`gcloud` through `output_with_timeout`) is given no more than the time
   left before the call's cap.
5. The retry behaviour above this layer is unchanged: a timeout is the same error kind as
   today's timeouts, so the loop's existing retry still applies.
6. Limits for tests: if the idle limit and cap cannot already be set to about a second from a
   test (environment variables read at call time, or an internal constructor), add the smallest
   internal way to do so and use it; do not change the production defaults (300 s idle, 1800 s
   cap) or the names of the environment variables.

No fallbacks: a timeout is an error, never an empty or partial response. Add no comments to
code. Nothing here needs to be resumable.

**Tests** (in `test_request_timeout.rs`, extending its local `TcpListener` harness; run with
`cargo test -p blyte-provider --test test_request_timeout`). Idle 1 s and cap 3 s unless stated.
Before changing any code, add tests 1 to 4 and run them: say in the final message which of them
failed on the unchanged code and how long each blocked (give each a guard so a hung call fails
the test after 20 s; it must not hang the suite).

1. `reused_connection_stalling_before_headers_times_out`: the server answers a first request
   with a complete, valid chunked streamed response and keeps the connection open, then reads
   a second request on the same connection and sends nothing for 30 s. Two calls through the
   shared pooled client with `req.timeout = None`: the first succeeds; the second returns a
   timeout error in under 3 s. Assert that the second request did arrive on the same
   connection; if the client opened a new one, the test fails with a message saying so.
2. `fresh_connection_stalling_before_headers_times_out`: accept, read the request, send
   nothing: timeout error in under 3 s.
3. `trickle_without_newline_hits_the_cap`: headers, then one space every 200 ms with no
   newline, for 30 s: timeout error between 2 s and 5 s.
4. `stall_after_first_chunk_times_out`: headers and one valid chunk, then nothing: timeout
   error in under 3 s after the chunk.
5. `slow_but_steady_stream_succeeds`: a valid response sent as five chunks 300 ms apart, with
   idle 1 s and cap 10 s: succeeds with the same content as when sent at once.
6. `request_timeout_shorter_than_cap_wins`: `req.timeout = Some(1 s)`, cap 10 s, the trickle
   server of test 3: timeout error in under 3 s.
7. `abandoned_helper_thread_ends`: after test 2's call returns, the server observes its
   connection closed by the client within cap + 10 s.
8. One of tests 2 or 4 repeated on the Vertex path, with the token source stubbed the way the
   existing Vertex tests stub it.

**Commit message:** `fix(provider): bound every Gemini and Vertex call by an idle limit and a whole-call cap`
