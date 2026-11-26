# Self-hosting gaps

A trial on 2026-11-25 had a blyte surface plan, dispatch and review one fix to blyte itself. It
showed four gaps. Two research runs (2026-11-26) established the facts below; each was checked
against the code.

## Facts

1. **The model can loosen a run's sandbox without the user.** The conversation model's `blyte`
   tool refuses only `--request`, `--surface` and `--started-by` on `dispatch`. It passes
   `--sandbox off`, `--network`, `--allow-read`, `--pass-env`, `--gate` and `--no-path-check`
   through. A run started with `--sandbox off`, or with `--allow-read` on a credential directory,
   can push with the user's credentials. The rule is that the model never pushes or opens a pull
   request without the user's say-so.
2. **Publishing already exists, for the user only.** The surface page has push and pull request
   buttons for a request (`POST /api/requests/<id>/push` and `/pull-request`,
   `requests::push_request`, `requests::open_request_pull_request`, `publish.rs`). The model
   cannot ask for them.
3. **A repository under a temp directory is writable by the model.** `conversation_allowed`
   allows writes under the system temp directories, so the write limits do not hold for a
   checkout there. Nine tests fail or would fail in such a checkout for this reason.
4. **Nothing tells the model when to stop reading and send a research run**, and a surface is
   never told that a research run finished (`run_results.rs` skips research runs).

## Decisions

- The model's `blyte` tool refuses every `dispatch` option that loosens the sandbox or replaces
  the gate. The user can still run those in their own terminal.
- The model may ask to publish a request. The page shows exactly what will be published; only
  the user's yes on that card makes the surface process push and open the pull request. The
  user chose this on 2026-11-26. Merging stays separate.
- The write limits hold wherever the checkout lives.
- Investigation needing more than about five file reads goes to a research run, and the surface
  is told when its research run ends.

No fallbacks anywhere in this plan: a missing value, a failed command or a state that does not
match is an error that names what is wrong. Nothing here is a long-running job, so "resumable"
does not apply; where an action has two steps (push, then pull request) the task says how a
retry continues.

## Tasks

| Task | What | Depends on |
|---|---|---|
| H1 | The `blyte` tool refuses options that loosen the sandbox | none |
| H2 | Write limits hold for a checkout under a temp directory | none |
| H3 | Research guidance, and a message when a research run ends | none |
| H4 | The model asks to publish; the user approves on the page | H1 |
| H5 | Instructions describe publishing as built | H3, H4 |

H1, H2 and H3 touch different files and run in parallel.

### Task H1: the blyte tool refuses options that loosen the sandbox

**Goal.** The conversation model must not be able to start a run with a weaker sandbox, extra
readable paths, extra environment variables, or its own gate.

**Files.** `crates/blyte-agent/src/tools.rs` (the `blyte` tool, where `--request`, `--surface` and
`--started-by` are refused for `dispatch`), `crates/blyte-cli/src/main.rs` (the
`MODEL_SHELL_ENV` refusals), and tests beside the existing ones for those refusals.

**What to build.**
- In the `blyte` tool, for `dispatch`: refuse any argument that is, or starts with the name and
  `=`, one of `--sandbox`, `--network`, `--allow-read`, `--pass-env`, `--gate`, `--no-path-check`;
  and the short form `-g` (both `-g` alone and `-g<value>`). The error is
  `do not pass <option>; only the user can loosen a run's sandbox or replace its checks`, with
  the option as written by the model up to any `=`. Nothing is spawned.
- In `main.rs`, when `MODEL_SHELL_ENV` is `1`, refuse `dispatch` with any of the same options,
  with the same message, exit code 2, before anything else runs. Decide from the parsed
  arguments where possible (a value different from the default, or a non-empty list), so that a
  form the string check would miss is still refused.
- `--model`, `--base`, `--plan`, `--plan-task`, `--tag`, `--research`, `--report-to`, the budget
  options, `--no-brief-check`, `--no-claim-check` and `--no-base-check` stay allowed.
- Put the list of refused options in one constant used by the tool.

**Tests.** One per refused option for the tool (the outcome is an error with the message, and no
process is spawned: use the existing test seam for the `blyte` tool), including `--sandbox=off`,
`--sandbox off`, `-g true` and `-gtrue`; one showing `--model x --research` is still accepted;
and for the CLI one test per option with `MODEL_SHELL_ENV=1` expecting exit 2 and the message,
plus one showing the same command without that variable is not refused by this check.

**Constraints.** Existing tests pass unmodified. No fallbacks.

**Siblings.** H2 (sandbox paths) and H3 (instructions, run results) run separately and are not
in this worktree. Do not edit `sandbox.rs`, `run_results.rs` or the files under
`crates/blyte-cli/assets/conversation/`.

**Commit message.** `fix: the model's blyte tool refuses dispatch options that loosen the sandbox`

### Task H2: write limits hold for a checkout under a temp directory

**Goal.** In a surface, the shell and the write tools may write only in `.wt/tasks`, an open
edit's worktree, and temporary directories. Today a repository that itself lives under a temp
directory is wholly writable, `.git` included. Make the limits hold there.

**Files.** `crates/blyte-agent/src/sandbox.rs` (`conversation_allowed` and its callers),
`crates/blyte-agent/src/shell.rs` (the conversation shell's environment), `crates/blyte-agent/src/tools.rs`
only if the write scope is built there from a separate list (find where `WriteScope.allowed` is
filled and keep it and the shell profile on one list), and tests.

**What to build.**
- After building the list in `conversation_allowed(root)`, compare each entry with the
  repository root, both canonicalised (so `/tmp` and `/private/tmp` are the same place). Remove
  every entry that is the root or an ancestor of it, except that `.wt/tasks` under the root is
  never removed. A path that cannot be canonicalised because it does not exist is compared as
  written; any other canonicalisation error is returned as an error naming the path
  (change the function to return `Result` and pass the error up to where the surface starts,
  which must then fail with it).
- When at least one entry was removed, the conversation gets a private temp directory:
  `<root>/.wt/tmp/<process id>`, created with mode 700, added to the allowed list, and set as
  `TMPDIR` for the conversation shell. Remove it when the surface exits. If it cannot be created,
  that is an error naming the path. When nothing was removed, behaviour is exactly as today and
  `TMPDIR` is untouched.
- The same list must drive both the OS sandbox profile (macOS and Linux) and the write tools'
  scope, so neither allows the root in this case.
- A dispatched run's sandbox is not part of this task.

**Tests.**
- Unit: with a root under a directory that is in the list (use a temp directory and `TMPDIR`
  pointing at its parent), the returned list contains no ancestor of the root and no entry equal
  to it, contains `<root>/.wt/tasks`, and contains the private temp directory; with a root
  elsewhere the list equals today's.
- Unit: `/tmp` given as a symlink alias of the root's real parent is removed too.
- macOS sandbox test (beside `conversation_profile_writes_only_in_wt_tasks_not_root_git_or_wt_passed`):
  with the repository created under the system temp directory (not `CARGO_TARGET_TMPDIR`), a
  sandboxed shell cannot write `<root>/root.txt` or `<root>/.git/forged`, can write
  `<root>/.wt/tasks/x`, and can write a file in `$TMPDIR`. Keep the existing early return for a
  nested sandbox (exit 71).
- The Linux equivalent beside the existing Linux test.
- The write-tool scope: with the root under a temp directory, a `write` to `<root>/src/x.rs`
  asks for approval instead of writing.

**Constraints.** Existing tests pass unmodified. No fallbacks: no silent keep of an entry that
could not be compared.

**Siblings.** H1 (the `blyte` tool's argument refusals in `tools.rs`) and H3 (instructions, run
results) run separately and are not in this worktree. Do not touch the `blyte` tool's argument
handling.

**Commit message.** `fix: a surface's write limits hold when the repository is under a temp directory`

### Task H3: research guidance, and a message when a research run ends

**Goal.** A surface's model read code for 25 turns instead of sending a research run, because
the instructions give no rule and it is never told when a research run ends.

**Files.** `crates/blyte-cli/assets/conversation/process.md`,
`crates/blyte-cli/assets/conversation/reference.md`, `crates/blyte-cli/src/run_results.rs`, and
the tests that cover those (search the tests for quoted lines of the two documents before
changing them, and update only what the new text makes false).

**What to build.**
- `run_results.rs`: the place that reports finished runs to their surface skips research runs
  (`!info.is_research`). Report them too. For a research run the message to the model says the
  run's id, its outcome, and the path of its report (`.wt/<id>.report.md`) when the outcome is
  `verified`; for any other outcome it says the outcome and the log path. This message is sent
  for every finished research run of this surface, passed or not (an implementer run still
  messages the model only when it did not pass). Each run is reported once, as today.
- `process.md`, section "Research": replace the optional sentence with a rule. Wording to use:
  "Read directly only to answer a question that takes about five file reads or fewer. Anything
  larger (how something works across files, where something is used, what a change would break,
  the cause of a defect you cannot place) goes to a research run: write the question to
  `.wt/tasks/<id>.md` with the sections Objective, Start from, Out of scope and Answer format,
  and run `blyte dispatch <id> .wt/tasks/<id>.md --research`. Ask narrow questions, several at
  once. You do not wait: blyte sends you a message when each one ends, with the path of its
  report. Check any claim you will act on against the code it cites."
- `process.md`, sections "Your part", "Direct edits" and "Hand the request on": make them agree
  with that rule. "Do not read the code first" stays for a request that is already clear enough
  to hand on; add that when it is not clear where or what the change is, the first step is a
  research run, not reading.
- `reference.md`, sections "While runs are going" and "Research runs": state that the model is
  messaged when a research run ends (always) and when an implementer run ends without passing,
  and that the report is at `.wt/<id>.report.md`.
- Do not change tool descriptions in `crates/blyte-agent/src/tools.rs` or `run.rs`.

**Tests.** In the tests for `run_results.rs`: a finished `verified` research run of this surface
produces one message naming the id and `.wt/<id>.report.md`; a research run that did not pass
produces one message naming the outcome and the log; a research run of another surface produces
none; a second pass over the same runs produces none. A test that the built-in process text
contains "about five file reads".

**Constraints.** No fallbacks. Keep both documents' other sections as they are.

**Siblings.** H1 (`tools.rs`, `main.rs`) and H2 (`sandbox.rs`, `shell.rs`) run separately and are
not in this worktree.

**Commit message.** `feat: a surface is told when its research run ends, and when to send one`

### Task H4: the model asks to publish; the user approves on the page

**Goal.** Let the conversation model ask for a request to be published, and have the surface
process do it only after the user approves that exact publish on the page. The model gains no
other way to push.

**Files.** `crates/blyte-agent/src/tools.rs` (a host-side tool seam like `PeerTools`),
`crates/blyte-agent/src/approval.rs`, `crates/blyte-cli/src/session.rs` (approval entries),
`crates/blyte-cli/src/serve.rs` (wiring), `crates/blyte-cli/src/requests.rs` and
`crates/blyte-cli/src/publish.rs` (reuse; pin the commit), `crates/blyte-cli/src/tty_approver.rs`,
`crates/blyte-cli/src/conversation_text.rs`, `crates/blyte-cli/assets/surface/index.html`, tests.

**What to build.**
- A tool `publish` with exactly `request` (a request id of this surface), `title` and `body`.
  It is listed only when a surface has registered the seam (as `tell` is). The model does not
  choose the branch, the remote or the base: the branch is the request's own branch, the remote
  is `origin`, the base is the default branch, exactly as the page's buttons do it
  (`requests::push_request`, `requests::open_request_pull_request`).
- Refusals before anything is asked, each an error to the model with the reason:
  - the turn was started by another surface's message (`blyte_started_by()` is set);
  - the request is unknown, not this surface's, still combining, or not in the state in which
    the page's push button is offered;
  - the title is blank, or longer than 200 characters; the body is longer than 20000 bytes;
  - a publish approval for this request is already waiting.
- The approval is structured, not a `(command, reason)` pair. Add a second method to `Approver`,
  `ask_publish(&self, details: &PublishDetails) -> Approval`, with
  `PublishDetails { request, remote_url, branch, base, commit, commits: Vec<String>, title, body }`
  where `commit` is the full SHA the branch points at when the tool is called and `commits` are
  the subject lines of the commits on the branch that are not on the base. `NoApprover` and the
  terminal approver return `Approval::Unavailable` for it. The `allow-unsandboxed` rules are never
  consulted for a publish.
- The session logs it as an approval with a kind: add an optional `publish` object (the details
  above) to the `approval_requested` entry; existing entries without it read as before. The
  answer goes through the existing `POST /api/session/approvals/<id>` with the surface token.
- The page shows a publish card, distinct from "Command outside the sandbox": heading
  "Publish request <id>?", then the remote URL, branch into base, the short commit, the list of
  commit subjects, the title, and the full body in a `<pre>`, all inserted as text (never as
  HTML), then the same yes/no buttons.
- On yes, in the surface process: check the branch still points at `commit`; if not, stop with
  an error to the model that says the branch moved after approval, and push nothing. Then push
  and open the pull request through the same functions the buttons use, so the same records
  (`.wt/combined/<request>.published.json`, the decision entries) are written. Return the pull
  request URL to the model. If the push succeeds and the pull request fails, say exactly that;
  a later `publish` of the same request, approved again, skips the push when the pushed commit
  already equals `commit` and only opens the pull request.
- On no: return "the user declined" to the model; nothing is pushed.
- `conversation_text.rs`, `SANDBOX_ON_PARAGRAPH`: replace the example `(for example `git push`)`
  with `(for example a package install)`, and add one sentence: "To publish a request, use the
  `publish` tool; it runs only when the user approves it on the page."

**Tests.**
- Tool: each refusal above; `publish` is not listed without the seam.
- Session and server, with the mock `Publisher`: a publish request logs an `approval_requested`
  entry with the `publish` details; answering yes calls push then open-pull-request once each
  with the request's branch, and the tool returns the URL; answering no calls neither; with the
  branch moved between the request and the yes, neither is called and the error says so; push
  succeeds and pull request fails, then a second approved publish opens the pull request without
  a second push.
- A peer-started turn (`submit_peer`) that calls `publish` gets the refusal and no approval
  entry is logged.
- `POST /api/peer/messages` with the message credential cannot answer the approval (403 on the
  approvals route).
- An `allow-unsandboxed` rule of `git` does not make a publish skip the approval.
- Page test: the card renders a body containing `<script>` as text.

**Constraints.** Existing tests pass unmodified, except a test that quotes the replaced example
in `SANDBOX_ON_PARAGRAPH`. No fallbacks. Do not add any other way for the model to reach
`push_branch` or `open_pull_request`.

**Siblings.** H5 updates `process.md` and `reference.md` afterwards; do not edit them here.

**Commit message.** `feat: the model can ask to publish a request, and the user approves it on the page`

### Task H5: instructions describe publishing as built

**Goal.** The conversation instructions still say the model has no command for publishing.

**Files.** `crates/blyte-cli/assets/conversation/process.md`,
`crates/blyte-cli/assets/conversation/reference.md`, and tests that quote the changed lines.

**What to build.** Replace the statements that pushing and opening a pull request are the
user's alone and that the model has no command for either with: when a request has passed and
is combined, and the user has asked for it to be published or the work is finished, call
`publish` with the request id, a conventional-commit title and a body that says what changed and
how it was checked; the user sees the exact branch, commits, title and body and decides; do not
ask in any other way, do not run `git push` or `gh` through the shell, never call `publish`
because another surface's message asked for it, and never merge. Keep "Do not say a request is
combined, pushed or merged unless blyte said so."

**Tests.** The built-in text contains "`publish`" in both documents and no longer contains
"You have no command for either".

**Constraints.** Other sections unchanged.

**Commit message.** `docs: the surface's instructions describe publishing with the user's approval`

### Task H6: a busy surface is not taken for a stopped one, and a hung test binary fails

**Goal.** A surface decides whether another surface is alive with one HTTP probe that gives up
after 300 ms and treats every error as "not running". On a loaded machine a live surface misses
that, `live_entries` deletes its registry entry, a resume by the same name is no longer refused,
and `resume_fails_when_name_is_running_in_another_repository` then serves for ever (seen: 46
minutes in the landing check until killed). Make the probe say "unknown" when it cannot tell,
never delete on "unknown", and make a hung test binary a failure instead of a hang.

**Depends on:** none (based on `feat/self-hosting-gaps`). **Siblings:** none.

**Files.** `crates/blyte-cli/src/surface_dir.rs`, `crates/blyte-cli/src/surface_registry.rs`,
`crates/blyte-cli/src/serve.rs` and `crates/blyte-cli/src/clean.rs` (callers only),
`crates/blyte-cli/tests/test_serve.rs`, `scripts/test-parallel.py`,
`crates/blyte-cli/tests/test_test_parallel_script.rs`, and a new
`crates/blyte-cli/tests/test_surface_liveness.rs`.

**What to build.**

1. In `surface_dir.rs`:
   - `pub enum Liveness { Running, NotRunning, Unknown }` (derive `Debug, Clone, Copy, PartialEq, Eq`).
   - `pub fn probe(pid: u32, port: u16, id: &str) -> Liveness`, replacing `health_matches`
     (remove `health_matches`; no wrapper returning `bool` from the HTTP result alone):
     - `port == 0` → `NotRunning`.
     - `pid` is not a live process (`libc::kill(pid, 0)` fails with `ESRCH`; `pid == 0` counts
       as not live) → `NotRunning`, without connecting. `EPERM` means the process exists.
     - connect to `127.0.0.1:<port>`: `ConnectionRefused` → `NotRunning`.
     - a complete HTTP response was read: status 200 with JSON `surface_id == id` → `Running`;
       any other complete response (other status, other id, not JSON, no header/body split)
       → `NotRunning` (something else owns the port).
     - anything else (connect, write or read timed out: `TimedOut`/`WouldBlock`; any other I/O
       error) → make one more attempt with a 2 s timeout for connect, read and write, classified
       by the same rules; if that attempt also cannot tell → `Unknown`. The first attempt keeps
       `PROBE_TIMEOUT` (300 ms), so the usual case costs what it does today.
   - `pub fn is_running(record: &SurfaceRecord) -> bool` returns true for `Running` **and**
     `Unknown` (a surface that might be alive is treated as alive: its name stays taken and
     `blyte clean` leaves it alone). Say so in a doc comment.
   - Any other use of the old function in `surface_dir.rs` (around the `pick_name`/running-name
     code) follows the same rule: `Unknown` counts as running.
2. In `surface_registry.rs`, `live_entries` keeps an entry whose probe is `Running` or
   `Unknown`, and removes the entry only on `NotRunning`. It passes the entry's `pid`.
3. `serve.rs` and `clean.rs` need changes only if they call the removed function directly.
4. `tests/test_serve.rs`, `resume_fails_when_name_is_running_in_another_repository`: the test
   must not be able to hang. Start a watchdog thread that sets the `stop` flag passed to
   `run_with_hooks` after 60 s (and is told to finish when the call returns), so that if the
   resume is wrongly accepted the call returns and the existing assertions fail. Do not change
   what the test asserts.
5. `scripts/test-parallel.py`: a new option `--timeout <seconds>` (integer, default `1200`,
   must be at least 1, otherwise exit 2 with a message naming the option). A binary still running
   after that long is killed (the whole process group: start it with `start_new_session=True`
   and `os.killpg(..., SIGKILL)`), counted as failed, and reported with the output it had
   produced plus a line `FAILED (timed out after <n>s): <executable>`. The run's exit code is 1
   as for any failure. Everything else about the script stays as it is.

No fallbacks: an error reading the registry stays an error; `Unknown` is never turned into
`NotRunning` to make progress. Nothing here runs for long, so "resumable" does not apply.

**Tests** (each must fail on the current code or be new).

`crates/blyte-cli/tests/test_surface_liveness.rs` (use `std::process::id()` for a live pid; for a
dead pid, spawn `true`, wait for it and use its pid):
- `port_zero_is_not_running`.
- `closed_port_is_not_running`: bind a listener on port 0, note the port, drop it, probe with
  the live pid → `NotRunning`.
- `dead_pid_is_not_running_without_connecting`: a listener that would answer with the right id,
  probed with the dead pid → `NotRunning`, and the listener accepted no connection.
- `right_id_is_running`: a thread answers `HTTP/1.1 200 OK` with body `{"surface_id":"abc"}`
  → `Running` for id `abc`.
- `other_id_is_not_running`: same server, probe for id `xyz` → `NotRunning`.
- `silent_listener_is_unknown`: a listener that accepts and never writes (hold the streams
  open until the test ends) → `Unknown`, and the call returns within 6 s.
- `slow_answer_is_running`: a server that answers with the right id after 800 ms → `Running`
  (proves the second attempt).
- `live_entries_keeps_a_stalled_entry_and_removes_a_dead_one`: in a temp registry directory
  write two entries with the registry's own write function: one with the live pid and the
  silent listener's port, one with the dead pid; `live_entries` returns only the first, the
  first's file still exists and the second's is gone.
- `is_running_is_true_when_unknown`: a `SurfaceRecord` pointing at the silent listener with the
  live pid → `true`.

`crates/blyte-cli/tests/test_test_parallel_script.rs`:
- `a_binary_past_the_timeout_is_killed_and_reported_failed`: one script that prints `started`
  and then sleeps 60 s, one that passes; run with `--timeout 2`; exit code 1, output contains
  `started` and `FAILED (timed out after 2s)`, the passing binary is reported as passing, and
  the whole run takes under 20 s.
- `a_timeout_below_one_is_an_error`: `--timeout 0` exits 2 and names `--timeout`.

Run: `cargo test -p blyte --test test_surface_liveness --test test_test_parallel_script --test test_serve`
and then the repository's checks.

**Constraints.** Existing tests pass unmodified apart from the watchdog in item 4 and callers
of the removed function. No new dependencies. No home-directory paths or private names.

**Commit message.**
`fix: a busy surface is no longer taken for a stopped one, and a hung test binary fails after a timeout`
