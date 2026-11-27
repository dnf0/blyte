# A resumed surface remembers, and a stopped surface can be found

`blyte --resume <name or id>` restarts a stopped surface and the page shows the earlier
conversation, but the model starts empty: its conversation
(`ReplSession.state.runtime.conversation`, a `Vec<blyte_provider::Message>`, with
`runtime.conversation_goal`) lives only in the process. The surface says so with
`RESTART_NOTICE_TEXT` ("the model does not remember them"). Separately, nothing tells the user how
to resume: a stopping surface prints nothing, and `blyte surfaces` lists only running ones.

Decisions:

- Save the model's own messages, not a summary rebuilt from `entries.jsonl`: the entry log keeps
  only 90 characters of each tool result, so a summary would lose everything the model read.
- Save by appending, one message per line, so a long conversation is not rewritten after every
  reply.
- Keep the file where the sandbox already refuses reads: the surface token directory
  (`blyte_agent::sandbox::surface_token_dir()`, the directory that holds `<id>.token`), not the
  surface's record directory, which sits inside the checkout. The messages contain whatever the
  model read.
- A prompt that was running when the surface stopped is not remembered; the page says so.

Rules for every task:

- No fallbacks: a saved conversation that cannot be read is an error that names the file and the
  line; it is never treated as an empty conversation.
- Resumable: this is the feature; no long job is added.
- No home-directory paths or private names in code, tests or messages.
- Change only the files your task names (plus `Cargo.lock` if needed).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && python3 scripts/test-parallel.py -- --workspace --all-targets`
- In tests, every wait on a surface, session or process uses a 30 s bound that returns as soon as
  its condition holds; no fixed sleep stands in for a condition.

### Task RM1

**Goal.** A resumed surface's model has the conversation it had when the surface stopped.

**Depends on:** none. **Siblings (not in this worktree):** RM2 adds `blyte surfaces --stopped` in
`surfaces_cmd.rs`, `cli.rs`, `main.rs`; RM3 later changes the page. Do not touch those.

**Files.** New `crates/blyte-cli/src/conversation_store.rs`; `crates/blyte-cli/src/lib.rs` (the
`mod` line); `crates/blyte-cli/src/session.rs`; `crates/blyte-cli/src/serve.rs`;
`crates/blyte-cli/src/clean.rs`; new tests `crates/blyte-cli/tests/test_conversation_store.rs`
and `crates/blyte-cli/tests/test_surface_resume_memory.rs`.

**What to build.**

1. *The file.* `<surface token dir>/<surface id>.conversation.jsonl`, created with mode `0o600`.
   Line 1 is `{"version":1}`. Every later line is exactly one of `{"model":"<name>"}`,
   `{"goal":<string or null>}`, `{"message":<blyte_provider::Message>}`.
2. *Saving.* After every prompt the runner finishes, whether it succeeded or failed, append: a
   `model` line if the model used differs from the last one recorded; a `goal` line if
   `conversation_goal` differs from the last one recorded; one `message` line for each message in
   the conversation that is not yet in the file. Flush and `sync_all`. Earlier bytes of the file
   are never rewritten, with one exception: if the in-memory conversation is shorter than the
   number of messages already saved, write the whole file again to a temporary file in the same
   directory and rename it over. If saving fails, the prompt's result is unchanged and the surface
   adds an error entry `blyte: could not save the conversation for resume: <reason>`.
3. *Loading, only on `--resume`.*
   - No file: start empty and keep today's `RESTART_NOTICE_TEXT` (a surface stopped by an older
     blyte, or one that never finished a prompt).
   - A final line with no newline is a write that was cut off: drop it and truncate the file to
     the last complete line.
   - Any other line that does not parse, a first line that is not `{"version":1}`, or an unknown
     key: the resume fails with exit code 1 and
     `blyte: cannot read the saved conversation <path> (line <n>): <reason>; delete that file to resume without the model's memory`.
     No surface record or registry entry may be left changed by the failed resume.
   - Trailing `ToolCall` messages that have no matching `ToolResult` after them are removed, in
     memory and in the file (rewrite by temporary file and rename).
   - Put the messages and the last `goal` into the runner's
     `state.runtime.conversation` / `conversation_goal` before the first prompt.
   - If the provider kind (`blyte_provider::detect_provider_kind`) of the model the surface is
     starting with differs from that of the last `model` line, replace the loaded messages with
     the single digest message that `blyte_agent::context::digest` produces for them (providers
     reject each other's tool-call records), and rewrite the file to match.
4. *What the page says on resume*, in place of `RESTART_NOTICE_TEXT`, when a conversation was
   loaded:
   - `blyte restarted: the model remembers the conversation above`
   - if the entry log has entries after its last `Idle` entry (a prompt was running when the
     surface stopped), instead:
     `blyte restarted: the model remembers the conversation above, except the last request, which was interrupted`
   - if the digest replaced the messages:
     `blyte restarted on a different kind of model: the model has a summary of the conversation above`
5. *A new surface* (no `--resume`) never reads a conversation file. If one exists for its id,
   that is an error naming the file.
6. *Cleaning.* Wherever `clean.rs` removes a stopped surface's token file or record, it removes
   that surface's conversation file too.

Find the existing seams rather than adding parallel ones: `AgentRunner::try_new` and
`run_with_first_output` in `session.rs`, `Shared::new_with_log` for the notice, `run_inner` in
`serve.rs` for the path and the resume flag.

**Tests.**

`test_conversation_store.rs`:
- saving twice then loading returns the messages, the goal and the last model in order;
- the second save leaves the file's earlier bytes identical and only appends;
- a file ending in half a line loads without it and is truncated to the last full line;
- a bad line in the middle is an error whose text contains the path and `line 3` (for a bad
  third line); so are a wrong version and an unknown key;
- a trailing tool call with no result is removed on load and from the file, while a complete
  call-and-result pair before it stays;
- the file's mode is `0o600` (unix);
- a conversation shorter than what is saved rewrites the file, and a load then returns exactly
  the shorter conversation.

`test_surface_resume_memory.rs`, using the same kind of local fake model server that existing
tests in this workspace use for provider requests (find one and follow it), with a real
`AgentRunner`:
- start a surface, send "the code word is heron", wait for idle, stop it; resume it, send "what
  is the code word?"; assert the request body the fake server received for the second prompt
  contains the first user text and the first reply; assert the entry log has the "remembers"
  notice and not `RESTART_NOTICE_TEXT`;
- resume with a conversation file whose second line is `not json`: exit code 1, the message
  above with the path and `line 2`, and the surface record's bytes unchanged;
- resume a stopped surface that has entries but no conversation file: `RESTART_NOTICE_TEXT`
  appears and the first request carries no earlier messages;
- a surface whose entry log ends with a user message and no `Idle` after it gets the
  "except the last request" notice on resume.

The first test must fail on the code as it is now.

**Commit message:** `feat(surface): a resumed surface's model remembers the conversation`

### Task RM2

**Goal.** `blyte surfaces --stopped` lists the stopped surfaces of the current folder.

**Depends on:** none. **Siblings:** RM1 changes `session.rs`, `serve.rs`, `clean.rs`, `lib.rs`;
RM3 later uses your collector for the page. Do not touch those files.

**Files.** `crates/blyte-cli/src/surfaces_cmd.rs`, `crates/blyte-cli/src/cli.rs`,
`crates/blyte-cli/src/main.rs`; new test `crates/blyte-cli/tests/test_surfaces_stopped.rs`.

**What to build.**

1. In `surfaces_cmd.rs`:
   ```rust
   #[derive(Debug, Clone, PartialEq, serde::Serialize)]
   pub struct StoppedSurface {
       pub id: String,
       pub name: String,
       pub last_used: u64,              // unix seconds
       pub last_message: Option<String>,
   }
   pub fn stopped_surfaces(folder: &Path, state_dir: &Path) -> Result<Vec<StoppedSurface>, String>
   ```
   `folder` is the main checkout for a git repository, or the folder itself otherwise, resolved
   the same way `serve` resolves it (reuse that code; do not copy it). Records come from
   `surface_dir::list_records_for_root`; a record whose surface is running
   (`surface_dir::is_running`) is left out. `last_used` is the `ts` of the last complete entry
   in the surface's `entries.jsonl`; when that file is absent or has no entries it is the
   record's `started_at` (documented: a surface that was started and never used was last used
   when it started). `last_message` is `surface_registry::first_line` of the last `UserMessage`
   entry's text, or `None` when there is none or that line is empty. A final line cut off
   without a newline is ignored; any other unreadable line is an error naming the file and line.
   Sorted by `last_used`, newest first, then by id.
2. `blyte surfaces --stopped`: prints only the stopped surfaces of the current folder, as a table
   `NAME  ID  LAST USED  LAST MESSAGE` (`LAST USED` as `YYYY-MM-DD HH:MM` UTC; an absent message
   as `-`; the message cut to 60 characters), then a blank line and
   `resume with: blyte --resume <name or id>`. With none: `no stopped surfaces in <folder>`.
   With `--json`: a JSON array of `StoppedSurface`. Without `--stopped` the command's output is
   unchanged, byte for byte.

**Tests** (`test_surfaces_stopped.rs`; build records and entry logs on disk with the crate's own
writers, in a temporary state directory):
- two stopped surfaces come back newest first with the right `last_used` and `last_message`;
- a running surface (a record whose pid and port answer the health check, as other tests in the
  crate set up) is left out;
- a surface with no entry log uses `started_at` and has no message;
- a log ending in half a line is read without it; a bad complete line is an error containing
  the file path and the line number;
- the table output and the `--json` output for a fixed case, compared exactly;
- `blyte surfaces` without `--stopped` prints what it printed before for the same state;
- a folder that is not a git repository works the same way.

**Commit message:** `feat: blyte surfaces --stopped lists the stopped surfaces of this folder`

### Task RM3

**Goal.** A stopping surface says how to resume it, and the switcher in the page lists stopped
surfaces.

**Depends on:** RM1 and RM2 (both are in this worktree's base).

**Files.** `crates/blyte-cli/src/serve.rs`, `crates/blyte-cli/src/surfaces_cmd.rs`
(`surfaces_payload` only), `crates/blyte-cli/assets/surface/index.html`; tests in
`crates/blyte-cli/tests/test_surface_page.rs` and a new
`crates/blyte-cli/tests/test_surface_stop_line.rs`.

**What to build.**

1. When `serve_until` returns `Ok(())` in `run_inner` (a clean stop), print to `out` and flush:
   `blyte: stopped surface <name>; resume with: blyte --resume <name>`.
   Nothing is printed for a start that failed.
2. `GET /api/surfaces` gains a `"stopped"` array next to `"surfaces"`: the `StoppedSurface`
   values from `surfaces_cmd::stopped_surfaces` for this surface's folder. If collecting them
   fails, the route answers 500 with the error text, as its other failures do.
3. In `index.html`: `validateSurfaces` requires `stopped` to be an array of objects with string
   `id`, string `name`, number `last_used`, and string-or-null `last_message`, and rejects
   anything else the way it rejects a bad `surfaces` array. `renderSurfaceList` shows, below the
   running surfaces and only when there are stopped ones, a heading `Stopped in this folder` and
   one row per stopped surface: its name, when it was last used (local date and time), the last
   message if any, and the command `blyte --resume <name>` in a code element with a button
   `Copy` that copies the command. A stopped row is not a link. All text set with `textContent`.

**Tests.**
- `test_surface_stop_line.rs`: start a surface, stop it, assert `out` ends with the stop line for
  its name; a start that fails prints no stop line.
- an HTTP test (in the new file): with one stopped surface in the folder, `GET /api/surfaces`
  returns it in `stopped` with its name, id, `last_used` and `last_message`, and the running
  surface is not in `stopped`.
- `test_surface_page.rs`, in the style of the tests already there: `validateSurfaces` accepts a
  payload with a valid `stopped` array and rejects one whose entry lacks `name`, and one with no
  `stopped` key; `renderSurfaceList` given one stopped surface renders the heading, the name, and
  the exact command text, and renders no heading given none.

**Commit message:** `feat(surface): a stopping surface says how to resume it, and the switcher lists stopped surfaces`
