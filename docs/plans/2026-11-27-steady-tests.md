# Tests that fail only on a busy machine, and the startup-order bug behind some of them

Five test binaries pass alone and sometimes fail in the full parallel run
(`python3 scripts/test-parallel.py -- --workspace --all-targets`, four binaries at once, each
running its tests on every core). A research run found the causes; each was checked against the
code.

**Product bug.** `crates/blyte-cli/src/serve.rs` (`run_inner`) advertises a surface before it can
answer: it binds the port, writes the surface record and the registry entry, and only then starts
the session (which runs `git` several times), prints the URL and starts accepting requests. While
it is in that window, any other `blyte` that lists surfaces connects (the kernel accepts for the
bound port), gets no answer to `/api/health`, and waits `PROBE_TIMEOUT` (300 ms) then
`PROBE_RETRY_TIMEOUT` (2 s) in `crates/blyte-cli/src/surface_dir.rs` for each such surface. A user
starting two surfaces at once, or running `blyte surfaces` during a start, hits it.

**Test defects.** Deadlines of 250 ms to 5 s on work that starts processes; fixed sleeps standing
in for a wait on a condition; a child process inheriting the test binary's state directory;
surfaces started by a test and never stopped.

Rules for every task:

- No fallbacks: a wait that runs out fails the test with a message that says what was waited for
  and what was seen. No retry of a whole test, no `#[ignore]`, no serial-only test runs, no
  `--test-threads`.
- Resumable: not applicable (nothing here runs for long).
- A deadline is a bound on failure, not a delay: every wait returns as soon as its condition
  holds, so raising a deadline must not slow a passing test. Do not lengthen any sleep.
- Do not weaken what a test proves. Keep every existing assertion unless the task says to change
  it.
- Tasks run in parallel in separate worktrees. Change only the files your task names.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && python3 scripts/test-parallel.py -- --workspace --all-targets`

### Task FT1

**Goal.** A surface is not advertised as running until its session has started.

**Depends on:** none. **Siblings (not in this worktree):** FT2 to FT6 change test files only; do
not touch them.

**Files.** `crates/blyte-cli/src/serve.rs`; new `crates/blyte-cli/tests/test_surface_start_order.rs`.

**What to build.** In `run_inner`, reorder startup so that none of these happens before
`Session::try_start_with_log_io` has returned `Ok`:

1. `crate::surface_dir::write_record_in` for this surface (the record carries the pid and port
   that other processes probe);
2. `crate::surface_registry::write_entry` for this surface;
3. printing `blyte: surface <name> at <url>`.

The port may be bound before or after the session starts, but it must be bound before 1 to 3
(they carry the port). The session's log lives in the surface's record directory, so create that
directory before starting the session if it does not exist. Everything else keeps its present
behaviour and messages, including every refusal that happens before the bind (resume of a running
surface, duplicate names, and so on).

If the session fails to start: return 1 with the same message as now, write no registry entry,
write no record for a new surface (`list_records_in` must not return one), and leave a resumed
surface's existing record exactly as it was.

**Tests** (new file; reuse the approach of the helpers in
`crates/blyte-cli/tests/test_surface_without_repository.rs`: its own isolated state directory,
the entry point that takes a runner factory, a stop flag, a 30 s deadline on every wait):

- `nothing_advertises_the_surface_while_its_session_is_starting`: the runner factory, when called,
  records what `surface_registry::live_entries` (or the registry directory's files) and
  `list_records_in` for the checkout return at that moment. Assert both were empty then, and that
  after the URL is printed both contain exactly this surface.
- `a_failed_session_start_leaves_no_record_and_no_registry_entry`: the factory returns `Err`.
  Assert the run returns 1, the registry directory holds no entry, and `list_records_in` is empty.
- `a_failed_resume_leaves_the_stopped_record_unchanged`: start a surface, stop it, read its record
  file's bytes; resume it with a factory that returns `Err`; assert the bytes are identical and
  the registry holds no entry.

The first test must fail on the code as it is now (confirm it) and pass after the change.

**Commit message:** `fix(surface): a starting surface is not listed until its session has started`

### Task FT2

**Goal.** `crates/blyte-cli/tests/test_serve.rs` passes on a busy machine.

**Depends on:** none. **Siblings:** FT1 reorders surface startup in `serve.rs`; your tests must
pass with the present order and must not depend on it. Do not change product code.

**Files.** `crates/blyte-cli/tests/test_serve.rs` only.

**What to change.**

1. One constant, `const WAIT: Duration = Duration::from_secs(30);`. Use it for every wait on a
   surface start, a session going idle, an entry arriving, or a combine finishing: the 5 s
   deadlines in `start_stoppable_surface_with_args`, `start_surface_and_read_url`,
   `start_stoppable_surface_with_runner`, `make_busy_surface`, `message_then_events`,
   `report_attachment_is_read_from_the_run`, `send_back_reaches_the_conversation`,
   `events_survive_a_restart`, `registry_doing_tracks_running_turn_first_line_and_returns_to_idle`,
   `a_peer_message_reaches_the_runner_marked`, the combine wait in
   `a_runs_changed_post_combines_without_waiting_for_the_timer` (and its message, which says
   "within 5 s"), and the two `recv_timeout(Duration::from_secs(2))` in
   `approval_endpoint_answers_validates_and_requires_secret`. Search the file for any other
   deadline under 10 s on such a wait and use `WAIT` there too. A timeout that is the thing under
   test (a test asserting that something times out, or that nothing arrives) stays as it is.
2. Every timeout failure message says what was waited for and prints what was seen (the surface's
   stdout and stderr so far, or the last response).
3. `start_surface_and_read_url` starts surfaces that never stop, so three live surfaces stay in the
   shared registry for the rest of the binary. Make its three callers
   (`secret_never_leaks_and_origin_check_stays`, `two_starts_give_different_secrets` twice) stop
   and join their surfaces before returning, by using the stoppable helper, and delete
   `start_surface_and_read_url` if nothing uses it.
4. `bare_blyte_starts_the_surface_without_a_terminal` spawns `blyte` with a scratch `HOME` but lets
   it inherit the test binary's `BLYTE_STATE_DIR` and `XDG_STATE_HOME`. Add
   `.env_remove("BLYTE_STATE_DIR").env_remove("XDG_STATE_HOME")`, as
   `surface_keeps_token_out_of_wt_and_rejects_unauthenticated_actions` does. Raise its 10 s exit
   deadline to `WAIT`.
5. `a_runs_changed_post_combines_without_waiting_for_the_timer` sleeps 250 ms after the surface
   starts, assuming the reconcile timer's first pass (it runs at once on start; the next is 30 s
   later) has finished. If that pass runs late, it combines `NC1` and the "not combined before the
   POST" assertion fails. Replace the sleep with a wait on something the test can observe without
   changing product code that shows the first pass has finished. If nothing observable exists,
   leave the sleep as it is and say so, with what you looked at, in your final message. Do not
   lengthen it.

**Tests.** The file's own tests, all still present. Run
`cargo test -p blyte --test test_serve` three times; all three must pass.

**Commit message:** `test: test_serve waits on conditions with a 30 s bound and stops the surfaces it starts`

### Task FT3

**Goal.** `crates/blyte-cli/tests/test_surface_without_repository.rs` passes on a busy machine.

**Depends on:** none. **Siblings:** as FT2. Do not change product code.

**Files.** `crates/blyte-cli/tests/test_surface_without_repository.rs` only.

**What to change.** One constant `const WAIT: Duration = Duration::from_secs(30);` used for the
5 s deadline in `wait_for_surface`, the 5 s read timeout in `http_request`, the 5 s deadline in
`wait_for_idle_after_seq`, the 5 s wait for the registry branch in
`branch_appears_after_git_init_without_restart`, and any other deadline under 10 s on a surface
start, a response, or a session going idle. Each timeout message says what was waited for and
prints what was seen (stdout and stderr so far, the last response, or the registry entry's
contents). Waits keep polling and return as soon as the condition holds.

**Tests.** The file's own tests. Run `cargo test -p blyte --test test_surface_without_repository`
three times; all three must pass.

**Commit message:** `test: the plain-folder surface tests wait up to 30 s and say what they saw`

### Task FT4

**Goal.** `test_persistent_shell` and `test_gate_slots` (both in `crates/blyte-agent/tests/`) pass
on a busy machine.

**Depends on:** none. **Siblings:** FT1 to FT3, FT5, FT6 touch other files. Do not change product
code.

**Files.** `crates/blyte-agent/tests/test_persistent_shell.rs`,
`crates/blyte-agent/tests/test_gate_slots.rs` only.

**What to change.**

1. `a_timeout_kills_the_command_and_keeps_its_output`: the 250 ms timeout also has to cover
   starting the shell, so on a busy machine the command is killed before `echo start` runs. First
   run `p.exec_user("true", &root, T).unwrap();` so the shell is already up, then give the command
   under test a 2 s timeout and expect `[killed by blyte after 2s]`. Keep the other assertions.
2. `a_background_job_does_not_hold_the_command`: start the shell first in the same way, start the
   clock after that, run `sleep 30 & echo started`, and assert the call returned in under 15 s
   (a call that waited for the job would take 30 s). Kill the background job before the test ends
   if the shell does not already do so on drop.
3. `one_slot_makes_the_second_gate_wait_for_the_first`: the main thread sleeps 400 ms and assumes
   the second thread was already waiting for all of it. Use two channels instead. The second
   thread sends on the first just before `acquire(1, &d, POLL)` and sends `waited` on the second
   just after it returns. The main thread: receives the first with a 10 s `recv_timeout`; asserts
   that a 300 ms `recv_timeout` on the second times out (the second gate is blocked while the
   first is held); drops `first`; receives `waited` with a 10 s `recv_timeout`; asserts
   `waited >= POLL`. Remove the elapsed-time assertions this replaces.

**Tests.** The two files' own tests. Run each binary three times
(`cargo test -p blyte-agent --test test_persistent_shell`, `--test test_gate_slots`); all must pass.

**Commit message:** `test: the shell and gate-slot tests no longer depend on start-up speed`

### Task FT5

**Goal.** `crates/blyte-cli/tests/test_publish_tool.rs` passes on a busy machine.

**Depends on:** none. **Siblings:** as FT4. Do not change product code, and do not touch
`crates/blyte-agent/tests/test_publish_tool.rs`.

**Files.** `crates/blyte-cli/tests/test_publish_tool.rs` only.

**What to change.**

1. One constant `const WAIT: Duration = Duration::from_secs(30);` replacing the 5 s deadlines in
   `wait_for_publish_approval`, `wait_for_nth_publish_approval`, the "combine gate never started"
   wait in `publish_tool_refuses_unknown_other_surface_unready_combining_and_duplicate_pending`,
   and the "timed out waiting for peer turn" wait in
   `peer_started_turn_refuses_publish_and_logs_no_approval`.
2. Those waits block on the session (`session.entries_after(after, <time left, at most 200 ms>)`)
   instead of polling with a zero wait and a 10 ms sleep, and on timeout print the entries seen.
3. In the combining test, the gate command loops until a release file appears. Create that file
   from a guard's `Drop`, so a panic earlier in the test never leaves the gate's shell running,
   and shell-quote the paths put into the gate command.

**Tests.** The file's own tests. Run `cargo test -p blyte --test test_publish_tool` three times;
all three must pass.

**Commit message:** `test: the publish tests wait on the session with a 30 s bound`

### Task FT6

**Goal.** Four more test files that start surfaces use the same 30 s bound.

**Depends on:** none. **Siblings:** as FT4. Do not change product code.

**Files.** `crates/blyte-cli/tests/test_requests.rs`,
`crates/blyte-cli/tests/test_surface_in_place_runs.rs`,
`crates/blyte-cli/tests/test_attachments.rs`, `crates/blyte-cli/tests/test_tell.rs` only.

**What to change.** In each file, one constant `const WAIT: Duration = Duration::from_secs(30);`
for every deadline or read timeout under 10 s on a surface start, an HTTP response, a session
going idle, a prompt or request count, or a `recv_timeout` for a result. Each timeout message says
what was waited for and what was seen. Leave alone: fixed sleeps, and any timeout that is the
thing under test (for example `the_timer_thread_ends_with_the_server`, and the sleeps around
`RACE1`/`RACE2` in `test_requests.rs`).

**Tests.** The files' own tests; run each binary once alone and they must pass.

**Commit message:** `test: surface tests in four more files wait up to 30 s`
