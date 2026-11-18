# Harness speed: stall timing and quotes by reference

Date: 2026-11-18. Evidence: 492 `gemini-4-argon` runs (337 implementation, 155 research) from
the 21 days before, read from every repository's `.wt/*.report.json` and `.wt/*.log`.

## What the runs show

- Model calls are 59% of implementation wall time and 89% of research wall time.
- 5% of implementation turns and 7% of research turns stall for more than 40 s (more than three
  times what their tokens explain); that is 17-19% of model time. Logs hold 77 "model call timed
  out" retries, 50 of them after 300 s or more of silence (the idle timeout's default).
  `TurnUsage` records only `model_ms`, so the runs cannot say whether a slow call was silent
  from the start, went silent midway, or streamed slowly throughout. The idle timeout cannot be
  set from data until that is recorded. **Task ST records it.** The new default is a later
  change, made from the recorded data.
- In research runs, turns holding a single `write` or `edit` are 6% of turns and 26% of model
  time (131 s each on average): the model retypes the code it quotes. Most runs then need two
  or three gate attempts because retyped quotes do not match. 31% of research reads are of a
  file the run already holds whole and unchanged; the rules tell the model to "read the file
  again before quoting it". **Task QR lets the report name the lines and has blyte paste them
  in**, and drops that rule.

## Looked at and left alone

- Parallel runs do not slow each other: small-turn latency is the same with 1 run or 8.
- Context size barely moves the per-turn floor (8 s at 32k tokens, 11 s at 256k).
- Reading a file from line 1 in pieces is already handled (`tools.rs`, `test_read_from_top.rs`).
  In implementation runs only 1% of turns are reads of content already held.
- Preloading the files a task names would save about one turn per run. Not worth the code.
- Tests (the implementer's own runs plus the gate) are up to 36% of implementation wall time.
  The implementer's own runs do not duplicate the gate: of 1,219 test commands in 339 runs, at
  least 80% name specific test files (2.9 per run, 29 s each, 60% of them failing, so they are
  doing work), and full-suite runs take at most 3.5 hours against the gate's 25. The gate's
  time is the repository's own check. No task.

## Rules for both tasks

- No fallbacks: when something is missing or malformed, report it by name. Never substitute a
  default, a guess, an empty result or a silent skip.
- Existing tests pass unmodified, except where a task says a test's text must change.
- Check before committing, from the worktree root, all three must pass:
  `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `git commit --no-gpg-sign` and the exact message given. One commit. Do not push.
- The two tasks run at the same time in separate worktrees. Do not do the other task's work.

### Task ST: record first-data time and longest silence for every model call

**Depends on:** none. **Sibling:** Task QR (adds `quote_expand.rs`, edits
`check_report_quotes` and `RESEARCH_RULES`); it is not in this worktree, leave those alone.

**Goal.** For each turn, record how long the stream took to deliver its first data and the
longest silence in it, and the same for each failed attempt that was retried, so the idle
timeout can be set from real runs.

**Files.** `crates/blyte-provider/src/lib.rs`, `crates/blyte-provider/src/gemini.rs`,
`crates/blyte-agent/src/run.rs`, new `crates/blyte-provider/tests/test_stream_timing.rs`, new
`crates/blyte-agent/tests/test_turn_stream_timing.rs`, and any existing test that builds a
`TurnUsage { .. }` literal without `..Default::default()` (add the new fields there).

**What to build.**

1. In `crates/blyte-provider/src/lib.rs`:

   ```rust
   /// When a streamed model call's data arrived. A "data event" is one SSE `data:` line.
   #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
   pub struct StreamTiming {
       /// Milliseconds from the start of the call to its first data event; `None` if none arrived.
       pub first_data_ms: Option<u64>,
       /// The longest wait for data, in milliseconds: from the start of the call to the first
       /// data event, between consecutive data events, or from the last data event to the end
       /// of the stream (its end, an error, or a timeout).
       pub max_gap_ms: u64,
       /// Data events received.
       pub data_events: u32,
   }
   ```

   and on `trait Provider` a method with a default body:

   ```rust
   /// Timing of the most recent `complete` call on this provider, whether it succeeded or
   /// failed; `None` for a provider that does not stream or has made no call.
   fn last_stream_timing(&self) -> Option<StreamTiming> {
       None
   }
   ```

   Do not add fields to `Response` or `Usage`: many tests build them as full literals.

2. In `crates/blyte-provider/src/gemini.rs`:
   - Add `pub fn parse_stream_timed<R: std::io::BufRead>(reader: R, cap: Duration, started: Instant, timing: &mut StreamTiming) -> Result<Response, ProviderError>`.
     It is the current body of `parse_stream`, and it updates `*timing` as it goes, so that
     `timing` is correct on every return path, `Err` included: on each `data:` line (after the
     existing skip of blank, comment, `event:`, `id:`, `retry:` and non-`data:` lines) it sets
     `first_data_ms` if unset, raises `max_gap_ms` to the wait since the previous data event (or
     since `started` for the first), and increments `data_events`; on every exit from the
     function (end of stream, read error, cap exceeded, a parse or API error) it raises
     `max_gap_ms` to the time since the last data event (or since `started` if there was none).
     Use one small helper or a guard so no return path is missed.
   - `parse_stream` keeps its signature and calls `parse_stream_timed` with a throwaway
     `StreamTiming`.
   - `Gemini` gets a field `last_timing: Mutex<Option<StreamTiming>>` (initialised to `None`
     in every constructor). `complete` sets it to `None` at its start. When the HTTP call
     returns a response, it passes a local `StreamTiming` to `parse_stream_timed` and stores
     `Some(timing)` afterwards, on success and on error alike. When the HTTP call itself fails
     (no response to read), it stores
     `Some(StreamTiming { first_data_ms: None, max_gap_ms: <ms since started>, data_events: 0 })`.
   - `impl Provider for Gemini` overrides `last_stream_timing` to return the stored value.

3. In `crates/blyte-agent/src/run.rs`, `TurnUsage` gets three fields, each `#[serde(default)]`,
   with doc comments:

   ```rust
   /// Milliseconds to the first data of the model call that ended this turn's calling
   /// (`None`: the provider does not stream, or no data arrived).
   pub first_data_ms: Option<u64>,
   /// Longest silence, in milliseconds, in that same call (0 when the provider does not stream).
   pub max_gap_ms: u64,
   /// One entry per attempt of this turn's model call that failed and was retried, in order.
   #[serde(default, skip_serializing_if = "Vec::is_empty")]
   pub failed_attempts: Vec<FailedAttempt>,
   ```

   ```rust
   /// A model call attempt that failed and was retried.
   #[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
   pub struct FailedAttempt {
       /// Wall time of the attempt, in milliseconds.
       pub elapsed_ms: u64,
       /// Whether the error was a timeout (`ProviderError::is_timeout`).
       pub timed_out: bool,
       /// As in [`blyte_provider::StreamTiming`]; `None` and 0 when the provider reported none.
       pub first_data_ms: Option<u64>,
       pub max_gap_ms: u64,
   }
   ```

   In the model-call loop (the `let resp = loop { ... match self.provider.complete(&req) ...`
   block): keep a `Vec<FailedAttempt>` for the turn; in the branch that retries
   (`is_transient(&e) && attempt < max_retries`, where `report.provider_retries += 1`), push an
   entry built from `call_start.elapsed()`, `e.is_timeout()` and
   `self.provider.last_stream_timing()`. Every `TurnUsage` pushed for the turn (the success
   push and the `push_deadline_turn` closure) carries `first_data_ms` and `max_gap_ms` from
   `self.provider.last_stream_timing()` read after the last attempt, and the turn's
   `failed_attempts`.

**Tests.**

`crates/blyte-provider/tests/test_stream_timing.rs` (use a `BufRead` over a reader that sleeps;
`std::thread::sleep` never returns early, so lower bounds are safe; keep upper bounds loose):

- `timing_of_a_stream_with_a_pause`: three `data:` events, the reader sleeping 40 ms before the
  first and 120 ms before the third. Expect `data_events == 3`, `first_data_ms` in `40..2000`,
  `max_gap_ms` in `120..2000`.
- `timing_of_an_empty_stream`: a reader that sleeps 50 ms then ends with no data. The call
  returns whatever `parse_stream` returns for it today; `first_data_ms == None`,
  `data_events == 0`, `max_gap_ms >= 50`.
- `timing_is_filled_when_the_read_fails`: one `data:` event, then a reader that sleeps 60 ms
  and returns `io::ErrorKind::TimedOut`. The result is `Err`, `first_data_ms.is_some()`,
  `data_events == 1`, `max_gap_ms >= 60`.
- `comment_and_event_lines_are_not_data`: lines `: ping`, `event: x`, blank, then one `data:`
  event give `data_events == 1`.
- `parse_stream_matches_parse_stream_timed`: the same input gives equal `Response`s.

For the data events use the smallest valid chunk the existing tests in
`crates/blyte-provider/tests/test_gemini_wire.rs` use.

`crates/blyte-agent/tests/test_turn_stream_timing.rs`, with a scripted provider like those in
`crates/blyte-agent/tests/test_deadline_stop.rs`, implementing `last_stream_timing`:

- `turn_usage_carries_the_calls_stream_timing`: the provider reports
  `StreamTiming { first_data_ms: Some(7), max_gap_ms: 9, data_events: 2 }`; the run's first
  `turn_usage` entry has `first_data_ms == Some(7)`, `max_gap_ms == 9`, no `failed_attempts`.
- `a_retried_timeout_is_recorded_as_a_failed_attempt`: the first `complete` returns a transient
  timeout error (the kind the existing retry tests use) with timing
  `{ None, 300_000, 0 }`, the second succeeds with `{ Some(5), 6, 1 }`. The turn has one
  `failed_attempts` entry with `timed_out == true`, `first_data_ms == None`,
  `max_gap_ms == 300_000`, and its own `first_data_ms == Some(5)`, `max_gap_ms == 6`. Make the
  retry wait short the way the existing retry tests do.
- `a_provider_without_timing_records_none`: a provider that does not override
  `last_stream_timing` gives `first_data_ms == None`, `max_gap_ms == 0`.
- `old_reports_still_parse`: a `TurnUsage` JSON object without the three new keys deserialises,
  with `None`, `0` and an empty vector; a turn with no failed attempts serialises without a
  `failed_attempts` key.

**Constraints.** No change to timeouts, retries or any behaviour other than recording. No new
dependencies.

**Commit message:** `feat(agent): record first-data time and longest silence of every model call`

### Task QR: research reports quote code by reference

**Depends on:** none. **Sibling:** Task ST (adds `StreamTiming`, edits `TurnUsage` and the
model-call loop in `run.rs`, edits `gemini.rs`); it is not in this worktree, leave those alone.

**Goal.** A research report can say which lines to quote instead of retyping them. blyte
replaces each such line with the file's own text before it checks the report, so the quote is
exact, costs the model a few tokens, and needs no fresh read.

**Files.** New `crates/blyte-agent/src/quote_expand.rs`; `crates/blyte-agent/src/lib.rs` (one
`pub mod quote_expand;` line, in alphabetical order); `crates/blyte-agent/src/run.rs` (only
`check_report_quotes`); `crates/blyte-cli/src/dispatch.rs` (only `RESEARCH_RULES`); new
`crates/blyte-agent/tests/test_quote_expand.rs`; new
`crates/blyte-agent/tests/test_research_quote_directive.rs`; and
`crates/blyte-cli/tests/test_dispatch_research.rs` only if it pins rule text that changes.

**What to build.**

1. The directive. A line of `report.md` that, after trimming whitespace, is exactly
   `<<quote PATH:N-M>>` or `<<quote PATH:N>>`, where `PATH` is an absolute path with no
   whitespace and `N`, `M` are 1-indexed line numbers, inclusive. A line inside a fenced code
   block (between a line starting with three backticks and the next such line) is never a
   directive and is left alone.

2. `crates/blyte-agent/src/quote_expand.rs`:

   ```rust
   /// Most lines one directive may quote.
   pub const MAX_QUOTE_LINES: usize = 200;

   /// Replace every quote directive in `report` with the lines it names.
   /// Returns the new text and one problem per directive that could not be expanded.
   pub fn expand(
       report: &str,
       read: &dyn Fn(&std::path::Path) -> Result<String, String>,
   ) -> (String, Vec<String>)
   ```

   A directive that expands is replaced by exactly these lines (`EXT` is the path's file
   extension, or nothing when it has none; the quoted lines are the file's, verbatim, no line
   numbers added):

   ````text
   `PATH:N-M`
   ```EXT
   <line N>
   ...
   <line M>
   ```
   ````

   For the single-line form the citation is `` `PATH:N` ``. Every other line of the report
   comes back unchanged, including line endings: a report with no directive comes back
   byte-for-byte identical, with no problems. Expanding the result again changes nothing.

   A directive that cannot be expanded stays in the text exactly as written and adds one
   problem, `report.md:<line number in the input>: <the directive as written>: <reason>`, with
   these reasons:
   - a line that starts with `<<quote` (after trimming) but does not match the form:
     `not a quote directive; write <<quote /absolute/path:N-M>> alone on a line`
   - `PATH` is not absolute: `the path must be absolute`
   - `read` fails: `cannot read the file: <the error>`
   - `N` is 0, or `M < N`: `bad line range`
   - `M` (or `N`) is past the end: `the file has <count> lines`
   - more than `MAX_QUOTE_LINES` lines: `quotes <count> lines; the most is 200, quote the part that matters`

   No fallbacks: never expand a directive partially, never clamp a range, never drop a
   directive.

3. `crates/blyte-agent/src/run.rs`, in `check_report_quotes` only: after reading `report.md`,
   call `quote_expand::expand` with the same `self.fs.read` the function already uses. If the
   text changed, write it back to `report.md` with `self.fs.write`; if that write fails, add
   the problem `report.md: could not write the expanded quotes: <error>`. Run the existing
   checks on the expanded text. Return the expansion problems first, then the existing ones.

4. `crates/blyte-cli/src/dispatch.rs`, `RESEARCH_RULES`:
   - Replace the bullet that starts `- Quote code only from what you have just read.` with:
     `- To quote code, do not retype it: put a line ``<<quote /absolute/path:N-M>>`` on its own in report.md (at most 200 lines) and the harness replaces it with those lines of the file. You need the line numbers, not a fresh read of the file.`
     (single backticks around the directive in the real text).
   - In the bullet that starts `- The harness checks report.md:`, after the sentence ending
     `(`path:N-M`).`, add the sentence: `A code block you type yourself is checked the same way, so prefer the quote line.`

**Tests.**

`crates/blyte-agent/tests/test_quote_expand.rs`, with `read` backed by a `HashMap` of path to
text, one test per line:

- a range directive becomes the citation line and a fenced block with the exact lines, and the
  extension as the tag (`/r/a.py:2-3` gives a block tagged `py`);
- the single-line form;
- a path with no extension gives an untagged block;
- text before and after the directive is unchanged, and a report without directives is
  returned identical (include a report with no trailing newline and one with `\r\n` endings);
- a directive inside a fenced block is left alone and reports no problem;
- an indented directive (leading spaces) is expanded;
- expanding twice equals expanding once;
- each of the six failures above: the directive stays in the text, the problem names the
  report line number, the directive and the reason, and other directives in the same report
  still expand;
- 200 lines expand, 201 do not.

`crates/blyte-agent/tests/test_research_quote_directive.rs`, modelled on
`crates/blyte-agent/tests/test_research_stop_check.rs`: a research run whose model writes a
`report.md` containing one valid directive for a file in the read roots or workspace passes
its gate with `quote_problems == Some(vec![])`, and `report.md` on disk then contains the
quoted lines and no `<<quote`. A second test: a directive naming a missing file leaves a
problem containing `cannot read the file` in the feedback or in `quote_problems`, whichever the
existing tests in that file assert on.

Also confirm with a test in `test_quote_expand.rs` that the expanded block passes
`research_check::check` with no problems for a real temporary file (build `Sources` as
`research_check`'s own tests do).

**Constraints.** `research_check.rs` and `claim_check.rs` are not changed. No new dependencies
(`regex` is already one).

**Commit message:** `feat(research): reports quote code by reference and blyte pastes the lines`
