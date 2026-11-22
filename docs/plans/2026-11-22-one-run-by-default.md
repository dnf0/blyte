# One run by default: plan

Spec: `docs/specs/2026-11-22-one-run-by-default.md`. Three independent tasks; they touch different
files and can run at the same time.

### Task G1: the default process hands a request to one run and ends the turn

**Goal.** Rewrite blyte's default process for the agent the user talks to on the surface, so that
a request for work becomes one run and the agent does not wait for it. Why: the spec above. Today
the process tells it to decide how work splits, dispatch tasks, wait with `blyte wait` and react
when runs end; staying in its turn is where its cost goes, and splitting cost more than one run.

**Files.** `crates/blyte-cli/assets/orchestrator/process.md`,
`crates/blyte-cli/assets/orchestrator/reference.md`, and the tests that assert their text (find
them: `crates/blyte-cli/tests/test_orchestrator_*.rs` and any other test that reads these assets).

**What must be true when it is done.** `process.md` says, in plain imperative prose of about the
present length or shorter:
- **Your part**: you are who the user talks to. Answer questions yourself. For work, hand the
  request to one run and end your turn. blyte follows the run: it shows the user the run's report
  when it ends, its checks review it, it combines the result and runs the full check, and the user
  pushes and opens the pull request from the surface. You do not write the implementation, wait
  for a run, read diffs, re-run checks, combine, push or open pull requests. Keep for yourself:
  design decisions, unclear requirements (ask the user), anything touching credentials, deployment
  or other repositories.
- **Hand the request on**: write the task file as the user's request in their words, with any spec
  or document they named by path, any decision made in the conversation that the code does not
  settle, which behaviours need a test, and the exact commit message. Do not read the code first.
  Dispatch it once, in the background with both outputs redirected (the exact command form stays
  in `reference.md`), tell the user in one line that run `<id>` has started, and end the turn. Do
  not call `blyte wait`.
- **One run**: a request is one run, however many parts it has. Split work across several runs
  only when the user asks for that, or when a run stopped because its budget ran out and the user
  agrees to a split. (Keep the existing rules on how to split, shortened, for that case only.)
  Keep the sentence about `blyte plan <file>` for a request that names a plan: the plan's tasks
  are the split the user asked for.
- **A follow-up** to a request whose work has not been merged is a new run started from that
  request's combined branch: `--base blyte/<request>`.
- **When blyte tells you a run did not pass**: read `blyte review <id>` for why; record
  `--verdict redone` with the reason; dispatch it again when what was missing is clear, otherwise
  tell the user. **When blyte tells you a combine failed**: keep the present section.
- The opening paragraph's "measured basis" adds one sentence: on a three-task request
  (2026-11-22, six trials against three) splitting the work across runs cost more and took longer
  than one run, with the same result.
`reference.md` is changed only where it contradicts this (waiting on runs as the normal course;
`blyte wait` stays documented as a command, for the split case).

**Tests.** Update the tests that assert the old text. Add assertions that `process.md` contains
the instruction not to call `blyte wait` after dispatching, the `--base blyte/<request>` form for
a follow-up, and the two conditions for splitting. Run those test files, then the gate.

**Constraints.** No fallbacks. Text and its tests only: no Rust source outside tests. No names of
private repositories or companies. Depends on: none. Siblings: G2 (the server reports a finished
run) and G3 (a follow-up's combine) run beside this and are not in this worktree; describe their
behaviour as the bullets above state it.

**Commit message.** `feat(orchestrator): the default process hands a request to one run and ends the turn`

### Task G2: the surface shows a finished run's report, and tells the agent only when it did not pass

**Goal.** On the surface (`crates/blyte-cli/src/serve.rs`), when a run that belongs to a request
ends, blyte's code puts the run's own report into the conversation, with no model turn. When the
run did not pass, it also queues one message for the agent. Why: the agent no longer waits for
runs (the spec above), so nothing else tells the user a run has ended.

**Files.** New `crates/blyte-cli/src/run_results.rs` (registered in `lib.rs`);
`crates/blyte-cli/src/serve.rs` (call it from the same place the `/api/runs` handler calls
`requests::maybe_combine`); `crates/blyte-cli/src/session.rs` only if an entry cannot otherwise be
added; new `crates/blyte-cli/tests/test_run_results.rs`.

**What to build.**
- `report_finished_runs(surface) -> Result<(), String>`: for every non-research run whose
  `.wt/<id>.start.json` has a `request` and whose summary `.wt/<id>.json` exists, and which has no
  marker `.wt/<id>.reported.json`:
  1. add one entry to the session log, of the existing kind the page already shows for a notice
     (find the kind `/api/requests/<id>/push` uses for its result; no change to
     `assets/surface/index.html`), with the text: a first line
     `Run <id>: <outcome>, <turns> turns, USD <cost to 2 places>, <minutes> min`, then the
     summary's `final_message`. A `final_message` longer than 4000 characters is cut at a
     character boundary and followed by a line `… (the rest: blyte review <id>)`;
  2. when `outcome` is not `verified`, queue one message for the agent through
     `session.queue_for_request(request, …)`: `Run <id> did not pass: <outcome>.` followed by the
     summary's gate failures when it has any (last 20 lines), and
     `Read blyte review <id> and tell the user, or dispatch it again.`;
  3. then write the marker `.wt/<id>.reported.json` (`{"outcome": …, "ts": …}`), last.
  A step that fails returns `Err` naming the run and the file, and writes no marker, so the next
  call does it again. A summary that cannot be parsed, or lacks `outcome`, is an `Err` naming the
  file. Missing `turns`, `cost_usd` or `seconds` in a summary is an `Err` too, unless the existing
  summary type declares them optional, in which case that part of the first line is left out.
- The `/api/runs` handler calls it once per request for the page, before `maybe_combine`, and
  records an `Err` with `session.record_error` as it does for the combine trigger.
- Check that `blyte clean` and the dispatch-time clean-up remove `.wt/<id>.reported.json` with the
  run's other files; if they work from a list of suffixes, add it there (that file may then be
  changed too).

**Tests** (`tests/test_run_results.rs`, real temporary repositories, built as
`tests/test_requests.rs` builds them):
- a verified run with a request: one notice entry with the first line and the report, a marker,
  nothing queued for the agent; a second call adds nothing;
- a `gate_failed` run: the entry, and exactly one queued message naming the run and outcome; a
  second call adds nothing;
- a run with no `request`, a research run, and a run with no summary yet: nothing happens;
- a 5000-character `final_message` is cut to 4000 with the closing line;
- an unparseable summary: `Err` naming the file, no marker;
- through the router: `GET /api/runs` on a surface with one finished run adds the entry once
  across two calls.
Run: `cargo test -p blyte --test test_run_results`, then the gate.

**Constraints.** No fallbacks. Resumable: the marker is the only state; nothing is held in memory.
Do not change `assets/surface/index.html`, `requests.rs`, `combine.rs` or the orchestrator's
assets. Depends on: none. Siblings: G1 (instructions) and G3 (`combine.rs` tests) are not in this
worktree.

**Commit message.** `feat(surface): show a finished run's report; tell the agent only when a run did not pass`

### Task G3: a follow-up run based on another request's combined branch combines correctly

**Goal.** A follow-up to a request that has not been merged is dispatched from that request's
combined branch (`blyte dispatch <id> <task> --base blyte/<first request>`) and belongs to a new
request. Prove that `blyte combine <second request>` then gives a branch `blyte/<second request>`
that contains the first request's commits followed by the follow-up's own, runs the check on it,
and can be combined again with the same result; fix `combine.rs` if it does not.

**Files.** `crates/blyte-cli/src/combine.rs` (only if a test fails);
the existing combine tests' file under `crates/blyte-cli/tests/` or the unit tests in
`combine.rs`, whichever holds tests built on real temporary repositories.

**What must be true when it is done.**
- Test 1: request A has one run with one commit, combined (`blyte/A` exists). Request B has one
  run whose worktree was started from `blyte/A`'s tip and adds one commit. Combining B: state
  `combined`; `blyte/B`'s history is base, A's commit, B's commit, in that order; the record's
  `base` is `blyte/A`'s tip and its `commits` lists B's commit only; `blyte/A` is unchanged.
- Test 2: combining B a second time changes nothing (same branch tip, same record).
- Test 3: after A's branch gains a further commit from a second run combined into A, B's branch,
  already combined, is left as it was (B is based on the tip its run started from), and combining
  B again still gives the same result.
- If any of these fails on the current code, fix `combine.rs` so they hold, keeping every existing
  test passing unmodified, and say in the report which failed and why.

**Constraints.** No fallbacks. Resumable and idempotent as the existing combine is. Do not change
`requests.rs`, `serve.rs` or the orchestrator's assets. Depends on: none. Siblings: G1 and G2 are
not in this worktree.

**Commit message.** `test(combine): a follow-up run based on another request's branch combines onto it`
