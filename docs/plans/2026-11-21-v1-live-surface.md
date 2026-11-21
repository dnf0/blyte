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

## The interface between the server (S6) and the page (S7)

Every response is JSON unless stated. Every failure is a 4xx or 5xx with `{"error": "<text>"}`.

| Request | Response |
|---|---|
| `GET /api/session` | `{"session_id","repo","started_at","model","effort","effort_supported","busy","queued","last_seq"}` |
| `GET /api/session/events?after=<seq>&wait_ms=<n>` | `{"entries":[<entry>...],"last_seq":<n>}`; waits up to `wait_ms` (at most 25000) when there is nothing after `after` |
| `POST /api/session/messages` `{"text","reports":["<run id>"...]}` | `{"seq":<n>}` |
| `GET /api/models` | `{"models":[{"id","effort_supported"}...]}` |
| `POST /api/session/model` `{"model"}` | as `GET /api/session` |
| `POST /api/session/effort` `{"effort":"low"\|"medium"\|"high"\|null}` | as `GET /api/session` |
| `GET /api/runs` | the `LiveRuns` of task S4, each finished run with one more field `hold` (`{"note","ts"}` or `null`) |
| `GET /api/runs/<id>/diff` | `text/plain`: the run's diff |
| `POST /api/runs/<id>/decision` `{"action","note"}` | `{"decision":{"run","action","note","orchestrator_message"},"seq":<n>}`; `action` is `accept`, `send_back`, `redo`, `hold` or `mark_read` |
| `GET /api/map` | as `blyte mock` serves it |

An entry is `{"seq","ts","kind",...}` with `kind` one of `user_message` (`text`, `reports`,
`queued`), `turn` (`n`, `text`, `input_tokens`, `output_tokens`, `cost_usd`), `tool_started`
(`name`, `input`), `tool_finished` (`name`, `summary`, `is_error`), `retry` (`attempt`, `max`,
`wait_ms`, `reason`), `error` (`message`), `idle` (`turns`, `cost_usd`), `model_changed`
(`model`), `effort_changed` (`effort`), `decision` (`run`, `action`, `note`).

### Task S4: the runs of this repository, as they are now

**Depends on:** none (the mock fixes are already on this branch). **Siblings:** S1, S2, S3 and S7
run at the same time in other worktrees; none of their code is needed. Do not read holds here
(`queue_actions.rs` is task S3's).

**Goal.** One function that gives the surface the current repository's runs as they stand now:
those in progress, and those finished, in the shape the mock's review queue already draws.

**Files.** `crates/blyte-cli/src/live_runs.rs` (exists, empty) and a new
`crates/blyte-cli/tests/test_live_runs.rs`. `crates/blyte-cli/src/replay.rs` only to make an
existing private function `pub(crate)` or to split `collect` so both callers share its code;
`collect`'s results must not change. No other file.

**What to build.**

```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct LiveRuns {
    pub generated_at: u64,
    pub repo: String,
    pub running: Vec<RunningRun>,
    pub runs: Vec<crate::replay::Run>,
    pub skipped: Vec<crate::replay::Skipped>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct RunningRun {
    pub key: String, pub id: String, pub research: bool, pub title: String, pub task: String,
    pub plan_file: Option<String>, pub plan_task: Option<String>, pub started_at: Option<u64>,
    pub turns: u64, pub gate_running: bool, pub activity: String,
}
pub fn collect(root: &Path, now: u64, days: u64) -> Result<LiveRuns, String>
```

- `root` is the repository's main checkout. Only its own runs are returned (`<root>/.wt/`), not
  those of other repositories on the machine. A `root` that is not a directory is an `Err`
  naming it. A repository with no `.wt` directory gives empty lists.
- `runs`: exactly what `crate::replay::collect` returns for this repository alone and the same
  `days`, built by the same code (titles, groups, flags, cards, verdicts, reports), newest
  `finished_at` first.
- `running`: the runs `replay::collect` reports as skipped with the reason `still running`, with
  their fields from the same job listing (`activity` is the job's current-activity text, `turns`
  and `gate_running` as the job has them), oldest `started_at` first. They are not in `skipped`.
- `skipped`: the remaining skipped entries, unchanged.
- `repo`: the name `replay` gives the runs of this repository.

No fallbacks: a file that cannot be read or parsed is reported the way `replay::collect` already
reports it (in `skipped`, with its reason), never dropped silently and never replaced with a
default. Add no comments to code. Nothing here needs to be resumable.

**Tests** (`cargo test -p blyte --test test_live_runs`), each on a temporary git repository with
hand-written `.wt/` files in the shape `test_replay.rs` writes them (reuse its helpers by
copying what is needed):

1. `finished_runs_match_replay`: two finished runs (one code, one research with a report file):
   `runs` equals, as JSON, the runs `replay::collect` gives for the same root.
2. `running_run_is_listed_not_skipped`: a run with a live status file in the shape the existing
   tests use for a running run: it is in `running` with its title and `turns`, and in neither
   `runs` nor `skipped`.
3. `other_repositories_are_left_out`: a second temporary repository with its own finished run is
   not in the result for the first.
4. `unreadable_run_is_reported`: a finished run whose summary file is not valid JSON is in
   `skipped` with a reason, and the other runs are still returned.
5. `no_wt_directory_is_empty`; `missing_root_is_an_error`.
6. `order`: `runs` newest first, `running` oldest first.
7. The existing replay tests pass unmodified.

**Commit message:** `feat(surface): the current repository's running and finished runs`

### Task S7: the page of the live surface

**Depends on:** none (the mock page with the report reader is already on this branch).
**Siblings:** S1 to S5 run at the same time in other worktrees. The server that answers this
page's requests is a later task (S6) and does not exist yet: build the page against the
interface written above, in the section "The interface between the server (S6) and the page
(S7)" of this plan, which is copied here in full when this task is dispatched.

**Goal.** The page a developer works in: the conversation with the orchestrator and the review
queue on the left, the map on the right, all on live data from the session server.

**Files.** A new `crates/blyte-cli/assets/surface/index.html` (one file, inline style and
script, no external script, style or font) and a new
`crates/blyte-cli/tests/test_surface_page.rs` (reads the file with
`concat!(env!("CARGO_MANIFEST_DIR"), "/assets/surface/index.html")`). Do not change the mock
page or any other file.

**How.** Start from a copy of `crates/blyte-cli/assets/mock/index.html` and keep its look, its
layout (Design and Review tabs on the left, map with Runs, System and Data tabs on the right,
Wide), its review cards, its report reader and its map code. Remove everything that exists only
for the mock: the replay bar and clock, play and speed, the guide, the check-in, the record and
its buttons, the offer bar, the `skipped` and `session` counters, the repository picker, and any
code that reads `/api/replay`. The header shows `blyte`, the repository name, and the counters
`wip` (runs in progress) and `waiting` (groups with undecided results).

**What to build.**

1. **Start-up.** `GET /api/session`, then follow the log: request
   `/api/session/events?after=<last seq seen>&wait_ms=25000` again as soon as each response
   arrives. Every entry is drawn once, in `seq` order; reloading the page starts from `after=0`
   and redraws the whole conversation.
2. **The conversation** (Design tab), one element per entry, each with `data-seq`:
   - `user_message`: the developer's message; `with report <id>` when it has reports; a
     `queued` mark while it has not started.
   - `turn`: the orchestrator's text, drawn with the page's `renderReportMarkdown` (never as
     HTML from the model).
   - `tool_started`: one line with the tool name and the first line of its input, marked as
     running; the matching `tool_finished` (the next one with the same name) replaces the mark
     with its summary, in the error colour when `is_error`. Lines of consecutive tools are
     compact; the full input is under a `<details>`.
   - `retry`: `model call failed (<reason>); retry <attempt> of <max> in <seconds> s`.
   - `error`: the message, in the error colour.
   - `idle`: a quiet line: `<turns> turns` and the cost as `USD <amount>` to two decimals, or
     `cost not known` when it is `null`.
   - `model_changed`, `effort_changed`, `decision`: one quiet line each.
   The view follows the newest entry unless the developer has scrolled up.
3. **Working state.** While the session is busy (from `GET /api/session` at start, then from
   the entries: busy after a `user_message`, not busy after an `idle` with nothing queued) the
   header shows `working` and the send button reads `Queue`; otherwise `Send`.
4. **Sending.** The message box posts to `/api/session/messages` with the attached reports'
   run ids (the chips from the report reader's `Use in conversation`). The message appears when
   its entry arrives from the log, not before. A refused request shows its `error` text beside
   the box and keeps the typed text.
5. **Model and effort.** Two selects in the header (`id="model-select"`, `id="effort-select"`),
   filled from `GET /api/models` and `GET /api/session`. Changing one posts it and applies the
   response. The effort select is disabled when `effort_supported` is false. A refused change
   shows its `error` text and puts the select back.
6. **The review queue.** `GET /api/runs` at start, every 5 seconds, and at once after a
   `decision` entry. Finished runs are drawn as the mock draws them (groups, cards, research
   items, the report reader). Runs in progress are listed above the groups, one line each: id,
   title, turn count, activity, `gate` when `gate_running`. A held run shows its note and stays
   in the queue under a `Held` heading at the end of its group.
   - `Accept`, `Redo`, `Mark read` post `/api/runs/<id>/decision` at once.
   - `Send back` and `Hold` open a one-line note field in the card (not a browser prompt); the
     decision is posted when the note is submitted; an empty note is not sent.
   - `Accept all clean` posts `accept` for each clean card of the group, one after another, and
     stops at the first refusal.
   - A refused decision shows its `error` text in the card. A card is removed or changed only
     from the next `/api/runs` response, never optimistically.
   - `Details` on a code run loads `/api/runs/<id>/diff` and shows it as text in the card.
7. **The map.** As in the mock, from `GET /api/map`; the Runs tab's counts come from
   `/api/runs`.
8. **Losing the server.** When a request fails at the network level, a bar across the top says
   `No answer from blyte. Trying again.` and the request is tried again after 1, 2, 4, then 8
   seconds (8 from then on); the bar goes when a request succeeds. Nothing stale is presented
   as current: while the bar is shown the queue is dimmed.

No fallbacks: an unknown entry `kind`, or a response missing a field this plan names, is shown
as an error line naming what was wrong (`unknown entry kind <kind>`); it is not skipped and no
default is substituted. Everything from the server is passed through `escapeHtml` before it is
placed in the page. Add no comments to code. Nothing here needs to be resumable.

**Tests** (`cargo test -p blyte --test test_surface_page`):

1. `page_has_its_parts`: the ids `conversation`, `conversation-form`, `model-select`,
   `effort-select`, `queue`, `map`, `report-reader`, `attached-reports`, `lost-server`.
2. `page_calls_every_route`: the strings `/api/session`, `/api/session/events`,
   `/api/session/messages`, `/api/models`, `/api/session/model`, `/api/session/effort`,
   `/api/runs`, `/decision`, `/diff` and `/api/map`.
3. `page_handles_every_entry_kind`: each of the ten kinds appears as a quoted string, and
   `unknown entry kind` appears.
4. `mock_only_parts_are_gone`: none of `/api/replay`, `guide-close`, `checkin-skip`,
   `Save record`, `Next arrival`.
5. `no_external_resources`: no `http://` or `https://` in a `src=` or `href=` attribute, no
   `@import`, and no `window.prompt(`, `alert(` or `confirm(`.
6. `no_raw_html_from_the_server`: the function that draws a `turn` (name it `renderTurn`) calls
   `renderReportMarkdown(`; the page contains `function escapeHtml`.

Also extract the page's inline script to a temporary file and run `node --check` on it; and, with
`node`, run the functions that draw entries (`renderEntry` for each of the ten kinds, and an
unknown kind) on hand-written entries and check each returns a string containing its `data-seq`.
Say in the final message what you ran and what it printed.

**Commit message:** `feat(surface): the page of the live surface, on the session server's interface`

### Task S6: the surface server, and bare `blyte` opens it

**Depends on:** S1, S2, S3, S4, S5 and S7 (all on this branch). **Siblings:** none running.

**Goal.** Running `blyte` with nothing else, in a terminal inside a repository, starts the live
surface: one process that holds one conversation with the orchestrator, serves the page of S7,
and answers the interface in the section "The interface between the server (S6) and the page
(S7)" of this plan. That section is part of this task: every request, response shape and entry
kind in it is the contract.

**Files.** `crates/blyte-cli/src/serve.rs` (exists, empty), `crates/blyte-cli/src/cli.rs`,
`crates/blyte-cli/src/main.rs`, a new `crates/blyte-cli/tests/test_serve.rs`, and, only where a
function you need is private, the smallest `pub` or `pub(crate)` change in `dispatch.rs`,
`mock.rs` or `session.rs` (no behaviour change there). Do not change
`assets/surface/index.html`, `http.rs`, `queue_actions.rs` or `live_runs.rs`.

**What to build.**

1. `serve.rs`:
   ```rust
   pub struct Surface { pub root: PathBuf, pub repo: String, pub session: crate::session::Session,
                        pub session_id: String, pub started_at: u64, pub days: u64 }
   pub fn route(surface: &Surface, req: &crate::http::Request, now: u64) -> crate::http::Response
   pub fn run(args: &SurfaceArgs, config: crate::repl::ReplConfig) -> i32
   ```
   `route` is a pure function of its arguments and the files under `root`, so tests call it
   without a socket. `repo` is the root directory's name. Routes:
   - `GET /` and `/index.html`: `include_str!("../assets/surface/index.html")`. Shared assets as
     `mock::route` serves them (`crate::dashboard::shared_asset`).
   - `GET /api/session`: `session.info()` plus `session_id`, `repo`, `started_at`.
   - `GET /api/session/events`: `after` is required and must parse as `u64` (else 400);
     `wait_ms` is optional, must parse as `u64`, and a value above 25000 is a 400, not clamped.
     Calls `session.entries_after`. `last_seq` is `session.info().last_seq` read after the wait.
   - `POST /api/session/messages`: body `{"text","reports"}`; `reports` may be absent (no
     attachments). For each run id, the attachment text is the file `<root>/.wt/<id>.report.md`;
     a run id that is not `[A-Za-z0-9._-]+`, or whose report file is missing or unreadable, is a
     400 that names the id and the path, and nothing is submitted. Then `session.submit`.
   - `GET /api/models`: `blyte_provider::discover_all_models()`; each model that was listed
     becomes `{"id","effort_supported"}` (the same rule `session.rs` uses for
     `effort_supported`); providers whose listing failed are returned beside it as
     `"unavailable":[{"provider","error"}]`, never dropped silently. No model listed at all is a
     502 with the providers' errors.
   - `POST /api/session/model`: first `blyte_provider::resolve_provider(Some(model))`; an `Err`
     is a 400 with the provider's message (this is where a missing key is reported) and the
     model is not changed. Then `session.set_model`. Responds as `GET /api/session`.
   - `POST /api/session/effort`: `session.set_effort`; its `Err` is a 400.
   - `GET /api/runs`: `crate::live_runs::collect(root, now, days)`. Before answering, for every
     finished run call `crate::queue_actions::ensure_checks(root, id)` and, if any returned
     `Ok(true)`, collect again. Each finished run gets `hold` from
     `crate::queue_actions::read_hold` (`{"note","ts"}` or `null`). Any `Err` from these is a 500
     with its text.
   - `GET /api/runs/<id>/diff`: the same diff `blyte review <id> --diff` prints for the run
     (reuse that code; do not shell out to `blyte`), as `text/plain`. An unknown run is a 404.
   - `POST /api/runs/<id>/decision`: body `{"action","note"}` with `action` one of `accept`,
     `send_back`, `redo`, `hold`, `mark_read` (anything else is a 400 listing them); `note` may
     be absent or null. Calls `crate::queue_actions::decide(root, id, action, note, now)`, whose
     `Err` is a 400 with its text, then `session.record_decision(run, action, note,
     orchestrator_message)` and answers `{"decision":{...},"seq"}` with `action` spelt as in
     the request.
   - `GET /api/map`: `crate::map::build(root, repo)`; its `Err` is a 422 as in the mock. No
     `repo` query parameter.
   - A known path with the wrong method is a 405; an unknown path a 404; a body that is not the
     JSON object the route needs is a 400 saying which field is missing or wrong. Every failure
     body is `{"error":"<text>"}`.
2. `run`: `root` is the top of the current git repository (`git rev-parse --show-toplevel`); not
   in a repository is an error that says so. The session is
   `Session::start(Box::new(AgentRunner::new(&config)), <model>)` with `orchestrate` forced on
   and the orchestrate defaults applied as `apply_orchestrate_defaults` does; the model is the
   one the terminal session would use for the same flags and environment. It binds
   `127.0.0.1:<port>` (`--port`, default 0, meaning the system picks), writes
   `<root>/.wt/surface.json` (`{"pid","port","session_id","started_at"}`) once listening,
   prints `blyte: surface at http://127.0.0.1:<port>/`, opens that address in the browser the
   way `blyte dashboard --open` does unless `--no-open` was given, and serves with
   `crate::http::serve`, passing each request to `route` with the current time. It removes
   `surface.json` when it stops on Ctrl-C (use the signal handling the crate already has; if it
   has none, leave the file and rely on the next rule).
   **A second `blyte` attaches.** Before starting, if `surface.json` exists: when
   `GET http://127.0.0.1:<port>/api/session` answers 200 with the same `session_id`, print
   `blyte: surface already running at <address>`, open the browser (unless `--no-open`) and exit
   0 without starting anything. When nothing answers on that port, or the answer has another
   `session_id`, the file is stale: say so on stderr, replace it and start. A `surface.json`
   that cannot be parsed is an error naming the file.
3. `cli.rs` and `main.rs`: `SurfaceArgs { port: u16, no_open: bool, days: u64 (default 14, must
   be 1 to 31) }` as top-level flags `--port`, `--no-open`, `--days`, and a top-level flag
   `--terminal`. With no subcommand and no prompt: when `--terminal` is given, or standard input
   is not a terminal, the terminal session runs exactly as today; otherwise `serve::run`.
   `--port`, `--no-open` or `--days` together with `--terminal`, a prompt or a subcommand is a
   usage error. `blyte --help` says in one line each what bare `blyte` and `--terminal` do.

No fallbacks: nothing is substituted for a failure, a missing file or an unknown value; every
failure is an error response or a non-zero exit that names what failed. Add no comments to
code. Nothing here needs to be resumable (the conversation is in memory in this version).

**Tests** (`cargo test -p blyte --test test_serve`). They build a `Surface` over a temporary
git repository and a `Session` started with a scripted `PromptRunner` (as `test_session.rs`
does), and call `route` directly:

1. `index_is_the_surface_page`: `GET /` is 200 HTML containing `id="model-select"`.
2. `session_info_shape`: exactly the nine keys of the interface.
3. `message_then_events`: `POST /api/session/messages` answers `{"seq":1}`; `GET
   /api/session/events?after=0&wait_ms=2000` returns the `user_message`, the runner's entries
   and `idle`, with `last_seq` equal to the last entry's `seq`.
4. `events_parameter_errors`: missing `after`, `after=x`, `wait_ms=25001`: each 400 with an
   `error` text naming the parameter.
5. `events_wait_returns_empty`: `after=<last_seq>&wait_ms=300` answers `{"entries":[]}` after
   at least 300 ms.
6. `report_attachment_is_read_from_the_run`: with `.wt/R1.report.md` holding `finding`, a
   message with `"reports":["R1"]` reaches the runner ending in `Research report R1:\nfinding`.
7. `missing_report_is_refused`: `"reports":["R9"]` is a 400 naming `R9` and the path, and no
   entry was added; `"reports":["../x"]` is a 400.
8. `runs_carry_hold_and_checks`: a finished run summary `.wt/T1.json` (shape as the tests of
   `live_runs` build it): `GET /api/runs` lists it with `"hold":null`; after
   `POST /api/runs/T1/decision {"action":"hold","note":"later"}` it has
   `hold.note == "later"`, and the response carried `decision.action == "hold"` and a `seq`
   whose entry is a `decision`.
9. `decision_errors`: an unknown action (400 listing the five), an unknown run (400 with the
   text of `decide`), `send_back` with no note if `decide` refuses that (assert what `decide`
   returns).
10. `send_back_reaches_the_orchestrator`: after `send_back` with a note, the runner received
    the `orchestrator_message` the response carried.
11. `effort_and_model_errors`: `POST /api/session/effort {"effort":"max"}` is a 400 with
    `effort must be low, medium or high`; `POST /api/session/model {"model":""}` is a 400.
12. `wrong_method_and_unknown_path`: `POST /api/session` is 405, `GET /api/nope` is 404, both
    with an `error` body.
13. `map_is_served`: `GET /api/map` on a repository with one Rust crate is 200 with a `parts`
    (or the map's own top-level) array that is not empty.
14. In `crates/blyte-cli/tests` beside the existing command-line tests:
    `terminal_flag_conflicts`: `blyte --terminal --port 1` exits non-zero with a usage error;
    `piped_stdin_still_runs_the_terminal_session`: the existing piped-input tests pass
    unmodified.
15. `second_start_attaches`: start `run` on a thread with `--no-open` in a temporary
    repository, read the port from `surface.json`, call `run` again with `--no-open`: it
    returns 0 and prints `surface already running`; with a `surface.json` pointing at a closed
    port it reports the stale file and starts.

**Commit message:** `feat(surface): bare blyte starts the live surface and serves its interface`
