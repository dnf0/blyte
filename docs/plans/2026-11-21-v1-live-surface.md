# Plan: the live surface, first version

Spec: `docs/specs/2026-11-21-v1-live-surface.md`. Branch: `feat/v1-live-surface`.

The modules `http`, `live_runs`, `queue_actions`, `session` and `serve` already exist as empty
files registered in `crates/blyte-cli/src/lib.rs`, so parallel tasks do not each edit `lib.rs`.

Rules for every task: no fallbacks (when something is missing, unknown or does not match, return
an error that names it; never substitute a default, a guess, an empty result or a silent skip).
Add no comments to code. Existing tests pass unmodified unless the task says otherwise. Nothing
in these tasks is a long-running job, so nothing needs to be resumable.

| Task | Files | Depends on |
| --- | --- | --- |
| S1 events for a live view | `blyte-agent/src/run.rs`, `blyte-cli/src/task_runner.rs` | none |
| S2 HTTP server | `blyte-cli/src/http.rs` | none |
| S3 queue decisions | `blyte-cli/src/queue_actions.rs`, `blyte-cli/src/dispatch.rs` | none |
| S4 live runs | `blyte-cli/src/live_runs.rs` | the mock fixes (PR #375) |
| S5 session worker and log | `blyte-cli/src/session.rs` | S1 |
| S6 routes and launch | `blyte-cli/src/serve.rs`, `cli.rs`, `main.rs` | S2, S3, S4, S5, S7 |
| S7 the page | `blyte-cli/assets/surface/index.html` | the mock fixes (PR #375) |

S4 to S7 are written out below once S1 to S3 are dispatched.

### Task S1: events a live view needs, and a runner that hands events to a callback

**Depends on:** none. **Siblings:** S2 (`http.rs`) and S3 (`queue_actions.rs`, `dispatch.rs`)
run at the same time in other worktrees; their code is not here and is not needed.

**Goal.** A page following a live orchestrator must see when a tool starts (a `blyte wait` can
run for minutes) and when a model call is being retried (today only printed to stderr), and
the session server needs the raw events, not lines formatted for a terminal.

**Files.** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/task_runner.rs`, a new test
file `crates/blyte-agent/tests/test_live_events.rs`, and any file where the compiler requires a
new `match` arm for the new variants (there, the new variants produce no output). No other file.

**What to build.**

1. Two variants on `blyte_agent::run::Event<'e>`:
   - `ToolStart { name: &'e str, input: String }`, emitted immediately before each tool call
     executes (before `tools::execute_with_deadline`), with the same abbreviated `input` that
     `Event::Tool` carries for that call.
   - `Retry { attempt: u32, max: u32, wait_ms: u64, reason: String }`, emitted wherever the
     loop is about to sleep and retry a model call, before the sleep. `reason` is the text the
     existing `eprintln!` prints for that retry. Keep the `eprintln!`.
   Each `Tool` event is preceded by exactly one `ToolStart` with the same `name` and `input`.
2. In `task_runner.rs`:
   `pub fn run_task_with_state_and_events(config: &TaskConfig, state: &mut blyte_agent::state::State, on_event: &(dyn Fn(&blyte_agent::run::Event<'_>) + Sync)) -> Result<(blyte_agent::run::Outcome, blyte_agent::run::RunReport), String>`.
   It does everything `run_task_with_state_and_writer` does (network-mode check, session id,
   provider and shell set-up, usage log, the run itself) except that it writes no formatted
   lines: every event the run emits is passed to `on_event`. Share the set-up code between the
   two functions; do not copy it. `run_task_with_state_and_writer` and
   `run_task_with_state_writer_and_progress` keep their signatures and their output, byte for
   byte, except that the terminal formatter prints nothing for the two new variants.
3. Neither function may call `std::process::exit`.

**Tests** (`cargo test -p blyte-agent --test test_live_events`), using a scripted provider in
the style of `crates/blyte-agent/tests/test_deadline_stop.rs`:

1. `tool_start_precedes_each_tool`: a script of one turn with two tool calls, then a turn with
   none; the collected events contain, in order, `ToolStart(a)`, `Tool(a)`, `ToolStart(b)`,
   `Tool(b)`, with equal `name` and `input` in each pair.
2. `retry_event_is_emitted_before_the_retry`: a provider whose first call returns the
   retryable error the loop already retries (use the same error kind the existing retry code
   matches) and whose second call succeeds; exactly one `Retry` with `attempt == 1`, a non-empty
   `reason`, and it comes before the first `Turn`. Keep the test fast: if the retry wait is not
   already configurable for tests, make it so in the smallest way and say how in the final
   message.
3. `no_retry_event_without_a_retry`: a script with no errors emits no `Retry`.

Add to the existing tests of `task_runner.rs` (or a new `crates/blyte-cli/tests/test_runner_events.rs`
if there are none that can drive a run without a network): a test that
`run_task_with_state_and_events` passes at least one `Turn` event to the callback. If the
function cannot be driven without a real provider, do not invent a provider override: say so in
the final message and leave this test out.

**Commit message:** `feat(agent): tool-start and retry events, and a runner that hands events to a callback`

### Task S2: a small HTTP server with request bodies

**Depends on:** none. **Siblings:** S1 and S3 run at the same time in other worktrees.

**Goal.** The session server needs `POST` with a body, real status codes, and handlers that may
block for a long poll. `dashboard::serve_http` is GET-only and reads no headers; leave it as it
is and build the new server in its own module.

**Files.** `crates/blyte-cli/src/http.rs` (exists, empty) and a new
`crates/blyte-cli/tests/test_http.rs`. No other file. No new dependency.

**What to build.**

```rust
pub struct Request { pub method: String, pub path: String, pub query: Vec<(String, String)>, pub body: Vec<u8> }
pub struct Response { pub status: u16, pub content_type: &'static str, pub body: Vec<u8> }
impl Request { pub fn query_value(&self, name: &str) -> Option<&str> }
impl Response {
    pub fn json(status: u16, value: &serde_json::Value) -> Response
    pub fn text(status: u16, body: &str) -> Response
    pub fn html(body: Vec<u8>) -> Response
}
pub fn serve<F>(listener: std::net::TcpListener, handler: F) -> std::io::Result<()>
where F: Fn(&Request) -> Response + Send + Sync + 'static;
```

- `serve` accepts connections until the listener fails; one thread per connection; one request
  per connection; every response carries `Content-Length`, `Connection: close` and
  `Cache-Control: no-store`. The handler may block for up to a minute; that must not delay
  other connections.
- `path` is the request target before `?`, percent-decoded. `query` is the pairs after `?`, in
  order, percent-decoded, with `+` as a space. A pair with no `=` has an empty value.
- The request line and headers are read with a 10 second socket read timeout. Headers are
  matched without regard to case.
- Errors, answered by `serve` itself without calling the handler, each with a plain-text body
  that says what was wrong:
  - a malformed request line, a bad percent escape, or headers over 16 KiB: `400`;
  - a method other than `GET` or `POST`: `405`;
  - `POST` without `Content-Length`: `411`; a `Content-Length` that is not a number: `400`;
    a body over 1 MiB: `413` (without reading it); a body shorter than `Content-Length`: `400`;
  - a `Host` header that is missing, or whose host is not `127.0.0.1`, `localhost` or `[::1]`
    (any port): `403`;
  - a `POST` whose `Origin` header is present and is not `http://127.0.0.1:<port>`,
    `http://localhost:<port>` or `http://[::1]:<port>` for the listener's own port: `403`.
  These two checks are what stops another website from driving the session; do not relax them.
- A handler that panics gives `500` with the body `internal error`; the server keeps serving.
- Reason phrases for `200, 201, 202, 204, 400, 403, 404, 405, 409, 411, 413, 422, 500`; any
  other status passed by a handler is a programming error: answer `500`.

**Tests** (`cargo test -p blyte --test test_http`), each binding `127.0.0.1:0`, running `serve`
on a thread, and speaking raw HTTP over a `TcpStream`:

1. `get_with_query`: `GET /a%20b?x=1&y=two+words&z` reaches the handler as path `/a b` and
   pairs `x=1`, `y=two words`, `z=`.
2. `post_with_body`: a JSON body arrives byte for byte; the handler's `202` is returned with
   the reason `Accepted`.
3. `post_without_length_is_411`, `oversized_body_is_413`, `short_body_is_400`,
   `put_is_405`, `bad_escape_is_400`.
4. `foreign_host_is_403`: `Host: example.com`. `missing_host_is_403`.
5. `foreign_origin_post_is_403`: `Origin: http://example.com`; and
   `own_origin_post_is_accepted` with the listener's port; and `get_ignores_origin`.
6. `slow_handler_does_not_block_others`: one request whose handler sleeps 1 s and a second,
   sent after it, that is answered in under 500 ms.
7. `panicking_handler_is_500_and_server_survives`: a second request after it succeeds.
8. `response_headers`: `Content-Length`, `Connection: close`, `Cache-Control: no-store`.

**Commit message:** `feat(http): a local HTTP server with request bodies and origin checks`

### Task S3: queue decisions as library functions

**Depends on:** none. **Siblings:** S1 and S2 run at the same time in other worktrees.

**Goal.** The surface's review queue must record real decisions. Today a verdict can only be
recorded through the `blyte review <id> --verdict` command path in `dispatch.rs`, review checks
are only computed there too, and a hold is not saved anywhere.

**Files.** `crates/blyte-cli/src/queue_actions.rs` (exists, empty), `crates/blyte-cli/src/dispatch.rs`
(only to move code out and call it), and a new `crates/blyte-cli/tests/test_queue_actions.rs`.
No other file.

**What to build**, all in `queue_actions.rs`. `root` is the repository's main checkout; run
files are under `<root>/.wt/`.

1. `pub fn record_verdict(root: &Path, id: &str, verdict: ReviewVerdict, notes: &str, now: u64) -> Result<(), String>`:
   exactly what `blyte review <id> --verdict` does today (write `.wt/<id>.verdict.json` with
   `verdict`, `notes`, `ts`, then the same clean-up call), moved here. The command path in
   `dispatch.rs` calls this function and prints and exits as before; its output and exit codes
   do not change. `ReviewVerdict` is the existing enum; use it from where it is defined.
2. `pub fn ensure_checks(root: &Path, id: &str) -> Result<bool, String>`: when the run has
   finished (`.wt/<id>.json` exists) and `.wt/<id>.checks.json` does not, compute the review
   checks with the same code `blyte review <id>` uses and write the file in the same format;
   return `Ok(true)` when it wrote the file, `Ok(false)` when the file was already there. A run
   that has not finished is an `Err` naming the run. Move the computation out of the command
   path so both use one function; `blyte review <id>` prints the same output as before.
3. Holds, in `.wt/<id>.hold.json` as `{"note": "...", "ts": <now>}`:
   `pub struct Hold { pub note: String, pub ts: u64 }` (serde),
   `pub fn set_hold(root, id, note, now) -> Result<(), String>` (an empty or whitespace note is
   an `Err`), `pub fn clear_hold(root, id) -> Result<(), String>` (no file is `Ok`),
   `pub fn read_hold(root, id) -> Result<Option<Hold>, String>` (no file is `Ok(None)`; a file
   that does not parse is an `Err` naming the path).
4. Decisions:
   ```rust
   #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum Action { Accept, SendBack, Redo, Hold, MarkRead }
   #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
   pub struct Decision { pub run: String, pub action: Action, pub note: Option<String>, pub orchestrator_message: Option<String> }
   pub fn decide(root: &Path, id: &str, action: Action, note: Option<&str>, now: u64) -> Result<Decision, String>
   ```
   Whether a run is a research run and what its outcome is are read from `.wt/<id>.json` the
   way `blyte review` reads them. Rules, each failure an `Err` with the quoted text
   (`<id>`, `<outcome>` filled in):
   - no `.wt/<id>.json`: `no finished run <id> in <root>/.wt` (with the real path);
   - `Accept` on a research run: `run <id> is a research run; mark it read`;
   - `Accept` when the outcome is not `verified`: `run <id> is <outcome>, not verified; it cannot be accepted`;
   - `MarkRead` on a run that is not a research run: `run <id> is not a research run`;
   - `SendBack` or `Hold` with no note or a whitespace note: `a note is required to send back run <id>` / `a note is required to hold run <id>`.
   Effects:
   - `Accept`: `record_verdict(Accepted, note or "")`; message
     `Accepted run <id>. Combine it onto the working branch and run the gate.`
   - `SendBack`: records no verdict; message
     `Send back run <id>: <note> Draft a follow-up task for me to confirm.`
   - `Redo`: `record_verdict(Redone, note or "")`; message `Redo run <id>.` followed by
     ` Reason: <note>` when there is a note.
   - `Hold`: `set_hold`; no message.
   - `MarkRead`: `record_verdict(Accepted, "read")`; no message.
   - Every action except `Hold` calls `clear_hold` after it succeeds.
   Notes are trimmed. `Decision.note` is the trimmed note, or `None`.

**Tests** (`cargo test -p blyte --test test_queue_actions`), each in a temporary directory made
into a git repository with a `.wt/` holding hand-written run files in the shape `blyte dispatch`
writes (copy the field names from an existing test that builds such files):

1. `accept_verified_code_run`: verdict file says `accepted`; the message is exactly as above.
2. `accept_refuses_research_run`, `accept_refuses_unverified_run`, `mark_read_refuses_code_run`,
   `unknown_run_is_an_error`: the exact error texts.
3. `send_back_needs_a_note` (none, and `"  "`); `send_back_records_no_verdict` (no verdict
   file afterwards; message carries the trimmed note).
4. `redo_records_redone` with and without a note (both message forms).
5. `hold_round_trip`: `set_hold`, `read_hold`, `clear_hold`, `read_hold` is `None`;
   `hold_needs_a_note`; `corrupt_hold_file_is_an_error`.
6. `decision_clears_hold`: hold, then `MarkRead` on a research run: no hold file.
7. `mark_read_records_accepted_with_note_read`.
8. `ensure_checks_writes_once`: first call `Ok(true)` and the file exists; second `Ok(false)`;
   `ensure_checks_refuses_unfinished_run`.
9. The existing tests of `blyte review --verdict` and `blyte review` pass unmodified.

**Commit message:** `feat(queue): record decisions, holds and review checks from library functions`

### Task S5: the session worker and its numbered log

**Depends on:** S1 (already on this branch). **Siblings:** S4 and S7 may run at the same time in
other worktrees (`live_runs.rs`, `assets/surface/`); none of their code is needed. The HTTP
routes are a later task (S6): build no server here.

**Goal.** One live conversation with the orchestrator, driven from other threads: messages go in,
and everything that happens comes out as numbered entries that a page can follow by asking for
"everything after entry N".

**Files.** `crates/blyte-cli/src/session.rs` (exists, empty), `crates/blyte-cli/src/repl.rs` (only
to move the building of `TaskConfig` for a prompt into a function both use), and a new
`crates/blyte-cli/tests/test_session.rs`. No other file.

**What to build.**

1. The log.
   ```rust
   #[derive(Clone, Debug, PartialEq, serde::Serialize)]
   pub struct Entry { pub seq: u64, pub ts: u64, #[serde(flatten)] pub body: EntryBody }
   #[derive(Clone, Debug, PartialEq, serde::Serialize)]
   #[serde(tag = "kind", rename_all = "snake_case")]
   pub enum EntryBody {
       UserMessage { text: String, reports: Vec<String>, queued: bool },
       Turn { n: u32, text: String, input_tokens: u64, output_tokens: u64, cost_usd: Option<f64> },
       ToolStarted { name: String, input: String },
       ToolFinished { name: String, summary: String, is_error: bool },
       Retry { attempt: u32, max: u32, wait_ms: u64, reason: String },
       Error { message: String },
       Idle { turns: u32, cost_usd: Option<f64> },
       ModelChanged { model: String },
       EffortChanged { effort: Option<String> },
       Decision { run: String, action: String, note: Option<String> },
   }
   ```
   `seq` starts at 1 and rises by one with no gaps; `ts` is seconds since the epoch. `cost_usd`
   is `None` only when the model has no known price (the existing price lookup says so); it is
   never a made-up number.
2. The runner, so that tests need no model:
   ```rust
   pub trait PromptRunner: Send + 'static {
       fn run(&mut self, prompt: &str, model: &str, emit: &(dyn Fn(EntryBody) + Sync)) -> Result<(), String>;
   }
   ```
   `pub struct AgentRunner` implements it over a `crate::repl::ReplSession` created with
   `orchestrate` on: it sets the goal and the model, builds the `TaskConfig` with the function
   moved out of `run_repl_stream` (same values as today), and calls
   `crate::task_runner::run_task_with_state_and_events`, turning `Event::Turn` into `Turn`,
   `Event::ToolStart` into `ToolStarted`, `Event::Tool` into `ToolFinished` and `Event::Retry`
   into `Retry`. Other events are not logged. It must never call `run_task` (that one can exit
   the process). `AgentRunner::new(config: &crate::repl::ReplConfig) -> Self`.
3. The session, a handle that is `Clone + Send + Sync`:
   - `Session::start(runner: Box<dyn PromptRunner>, model: String) -> Session` starts one worker
     thread that takes prompts in order.
   - `submit(&self, text: &str, attachments: Vec<Attachment>) -> Result<u64, String>` with
     `pub struct Attachment { pub run: String, pub text: String }`: a blank `text` is
     `Err("a message needs text")`. Appends `UserMessage` (`reports` = the attachments' run ids,
     `queued` = whether a prompt is running or waiting) and returns its `seq`. The prompt given
     to the runner is the text followed, for each attachment, by a blank line,
     `Research report <run>:` and the attachment's text.
   - While a prompt runs, the runner's entries are appended as they are emitted. When it returns
     `Ok`, `Idle` is appended with the number of `Turn` entries and the sum of their costs (`None`
     if any was `None`). When it returns `Err(e)`, `Error { message: e }` is appended and then
     `Idle`. A panic in the runner is caught: `Error` with the panic text, then `Idle`; the
     worker carries on with the next prompt.
   - `entries_after(&self, after: u64, wait: std::time::Duration) -> Vec<Entry>`: entries with
     `seq > after`; when there are none it waits up to `wait` for one (a `Condvar`, no polling
     loop) and returns an empty list if none came.
   - `info(&self) -> SessionInfo` with `pub struct SessionInfo { pub model: String, pub effort:
     Option<String>, pub effort_supported: bool, pub busy: bool, pub queued: usize, pub last_seq:
     u64 }` (serde `Serialize`).
   - `set_model(&self, model: &str) -> Result<(), String>`: a blank model is an `Err`; otherwise
     it is used from the next prompt and `ModelChanged` is appended. The current effort is kept
     when the new model accepts effort and dropped (with `EffortChanged { effort: None }`) when
     it does not.
   - `set_effort(&self, effort: Option<&str>) -> Result<(), String>`: accepted values are
     `low`, `medium`, `high` and `None`; anything else is
     `Err("effort must be low, medium or high")`. When the current model's provider does not
     take effort: `Err("<model> does not take an effort setting")`. Effort is carried the way
     the terminal session's model names carry it today (the `-low|-medium|-high` suffix handled
     by `blyte_provider::gemini::split_effort`); `info().model` is the name without the suffix.
     Appends `EffortChanged`.
   - `record_decision(&self, run: &str, action: &str, note: Option<&str>, orchestrator_message:
     Option<&str>) -> Result<u64, String>`: appends `Decision`, then, when there is a message,
     submits it as a prompt with no attachments. Returns the `Decision` entry's `seq`.
   - Dropping the last handle stops the worker after the prompt in progress.

No fallbacks: nothing is substituted for a failure; every failure is an `Err` or an `Error`
entry that says what failed. Add no comments to code. Nothing here needs to be resumable (the
log is in memory in this version).

**Tests** (`cargo test -p blyte --test test_session`), with a scripted `PromptRunner` that emits
given entries, can block on a channel until released, and can fail or panic on request:

1. `entries_are_numbered_without_gaps`: one prompt whose runner emits a turn, a tool start and a
   tool finish: `seq` 1 to 5 (`user_message`, `turn`, `tool_started`, `tool_finished`, `idle`).
2. `entry_json_shape`: `serde_json::to_value` of a `Turn` entry has exactly the keys `seq`, `ts`,
   `kind`, `n`, `text`, `input_tokens`, `output_tokens`, `cost_usd`, with `kind == "turn"`.
3. `messages_sent_while_busy_run_in_order`: with the runner blocked on the first prompt, submit
   two more: their entries have `queued: true`, `info().queued == 2`; after release the runner
   saw the three prompts in order and `info().busy` ends false.
4. `blank_message_is_refused`.
5. `attachments_are_appended_to_the_prompt`: the runner received the exact text described above;
   the entry lists the run id.
6. `runner_error_becomes_error_then_idle`; `runner_panic_becomes_error_then_idle_and_the_next_prompt_runs`.
7. `entries_after_waits_for_a_new_entry`: a call with `after = last_seq` and a 5 s wait returns
   within 1 s of a submit made 200 ms later from another thread; with nothing submitted and a
   300 ms wait it returns empty after at least 300 ms.
8. `idle_sums_turn_costs`, and `idle_cost_is_none_when_a_turn_has_no_price`.
9. `set_effort_rules`: on a `gemini-4-neon` model, `high` then `None`, with the entries and
   `info()` after each; an unknown value; a model whose provider takes no effort (use a model
   name the provider crate detects as Anthropic).
10. `set_model_keeps_or_drops_effort`.
11. `decision_with_a_message_submits_it`: `Decision` then `UserMessage`, and the runner got the
    message; without a message, only `Decision`.
12. The existing tests of the terminal session pass unmodified.

**Commit message:** `feat(session): a session worker with a numbered log that other threads can follow`
