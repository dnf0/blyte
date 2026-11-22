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

## Fewer runs of the check

Measured on two trials of the three-task request (2026-11-22): the full check ran three times per
request and took about 16 of 41 minutes: once on the untouched code before the model starts
(`base_check`, `crates/blyte-agent/src/run.rs`), once on the finished work (the gate), and once
on the combined branch (`combine.rs`). With one run on an unchanged base, the third tests the
same content as the second. The gate on the finished work stays as it is.

### Task H1: remember which content has passed the check, and do not check the same content again

**Goal.** Keep a record, per repository, of the git trees that have passed the check. Use it in
two places: `blyte combine` does not run the check when the combined branch's tree has already
passed, and `blyte dispatch` does not ask for the check on the untouched code when the base
commit's tree has already passed. Why: each avoided check is five to six minutes.

**Files.**
- New `crates/blyte-cli/src/passed_trees.rs`, registered in `lib.rs`.
- `crates/blyte-cli/src/combine.rs`, `crates/blyte-cli/src/dispatch.rs`.
- New `crates/blyte-cli/tests/test_passed_trees.rs`; existing combine and dispatch tests where
  they need a new assertion.

**What to build.**
1. `passed_trees.rs`:
   - `check_key(gate: &str, env: &BTreeMap<String, String>) -> String`: a hex SHA-256 of the gate
     text as configured in `blyte.toml` (without the comment step dispatch adds) and the
     `[dispatch.env]` pairs in sorted order. Two different gates or environments give two keys.
   - `record_passed(root, tree, key) -> Result<(), String>` writes
     `<main checkout>/.wt/passed/<tree>.<key>.json` with `tree`, `key` and `ts` (seconds), written
     to a temporary file and renamed. `root` may be a linked worktree: use
     `crate::config::main_checkout`.
   - `has_passed(root, tree, key, now) -> Result<bool, String>`: true only when that file exists,
     parses, names the same tree and key, and its `ts` is at most 7 days before `now`. A missing
     file is `false`. A file that exists and cannot be read or parsed is an `Err` naming it.
   - `tree_of(dir, rev) -> Result<String, String>`: `git rev-parse <rev>^{tree}`.
   - `worktree_is_clean(dir) -> Result<bool, String>`: `git status --porcelain` is empty.
2. `combine.rs`, in `run_combine_inner` where the gate runs today: after the cherry-picks, take
   the tree of the worktree's `HEAD`. If the worktree is clean and `has_passed` is true for that
   tree and the key of the gate and environment combine uses, do not run the gate; the record is
   `combined` as on a pass. Otherwise run the gate as today, and on a pass with a clean worktree
   call `record_passed`. Add to `CombineRecord` a field `check` with values `ran` and
   `already_passed` (serde default `ran`, so existing record files still parse), and make
   `blyte combine`'s printed line and `--json` output say which. `GET /api/requests` already
   passes the record through `requests.rs`: add `check` to what it returns only if that needs no
   change outside `requests.rs`; otherwise leave it and say so in the report.
3. `dispatch.rs`:
   - Where `base_check` is decided: when it would be true, take the tree of `base_sha`; if
     `has_passed` is true for it with the key of the configured gate and environment, do not pass
     `--base-check` to the child, write `"base_check": "already_passed"` where the start record
     has `"base_check"` today (keep `true`/`false` for the other two cases), and print one line
     to stderr: `base check: this commit's content already passed`.
   - After the child ends: if `--base-check` was passed and the summary's `turns` is at least 1
     (the base check runs before turn 1 and a failure ends the run as `blocked` with no turns),
     `record_passed` for the base commit's tree.
   - After the child ends with outcome `verified`: if the run's worktree is clean, `record_passed`
     for the tree of the worktree's `HEAD`. If it is not clean, record nothing.
   - A failure to write a record is reported on stderr and in the summary's `error`-free path as
     a warning line; it does not change the run's outcome. A failure to read one (`has_passed`
     returning `Err`) stops the dispatch with that error before the child starts.
4. Research runs never read or write these records.

**Tests** (real temporary git repositories, as the existing combine and dispatch tests use).
- `passed_trees`: a recorded tree is found; another tree is not; another key is not; a record 8
  days old is not; an unparseable record file is an `Err` naming the file; a linked worktree
  writes to the main checkout's `.wt/passed/`.
- combine: with a gate that appends a line to a file outside the repository each time it runs
  (so runs can be counted): a run's tip tree already recorded, one run, unchanged base → the gate
  does not run, state `combined`, `check` is `already_passed`; nothing recorded → the gate runs
  once, `check` is `ran`, and the combined tree is then recorded; two runs with nothing recorded
  for the combined tree → the gate runs; a recorded tree but a different gate text → the gate
  runs; an untracked file in the combine worktree → the gate runs.
- dispatch (use the existing way dispatch tests stand in for the child process; if there is
  none, test the decision as a function taking the start facts and returning whether to pass
  `--base-check`): base tree recorded → no `--base-check`, start record says `already_passed`;
  not recorded → `--base-check` as today; `--no-base-check` and `base_check = false` still mean
  no check and are written as `false`; after a verified run with a clean worktree the tip's tree
  is recorded; with an uncommitted change it is not; after a `blocked` run nothing is recorded.

Run: `cargo test -p blyte --test test_passed_trees` and the combine and dispatch test targets
you touched.

**Constraints.** No fallbacks: an unreadable record is an error, never treated as "not passed"
silently and never as "passed". Resumable: everything is in files under `.wt/passed/`. Existing
tests pass unmodified except where a new field needs an assertion. Do not change
`crates/blyte-agent`. Depends on: none. Siblings: H2 (running the base check beside the model)
is later and not in this worktree.

**Commit message.** `feat(check): do not check content that has already passed`

### Task H2: when the base commit has not passed before, check it beside the model's first turns

**Goal.** When H1 finds no record for the base commit, the check on the untouched code still
runs, but in its own worktree at the same time as the model's first turns, so the run does not
wait for it. If it fails, the run stops as `blocked`, as today.

**Files.** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/task_runner.rs`,
`crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/src/cli.rs`; tests beside the existing
`base_check` tests in both crates.

**What to build.**
1. `dispatch.rs`: when it would pass `--base-check`, create a detached worktree
   `.wt/<id>.base` at `base_sha` and pass `--base-check-dir <path>` as well. After the child
   ends, remove that worktree (`git worktree remove --force`), whatever the outcome.
2. `task_runner.rs`: with `--base-check-dir`, start the gate steps in that directory on a thread
   (`blyte_agent::run::run_gate_steps`, same shell, sandbox and environment as the run's gate,
   same machine-wide gate slot rule) before the first model call, and give the `Runner` a way to
   ask for its state.
3. `run.rs`: `base_check` becomes a choice of three: none, before the first model call (today's
   behaviour, kept for callers that pass no directory), or polled. Polled: at the start of every
   turn, and once more just before the run would return `Verified`, ask for the state:
   `Pending`, `Passed`, or `Failed { steps, tail }`. On `Failed`, return `Outcome::Blocked` with
   the same message as today. `Pending` never delays a turn. Just before returning `Verified`
   with the state still `Pending`, wait for it to finish (the run's own gate has passed on the
   changed tree; the base result is needed only to record it) and treat `Failed` there as
   `Passed` for the outcome but report it in the final message as
   `the check failed on the untouched code but passes with this change`.
4. When the run ends for any other reason while the base check is `Pending`, stop its process
   group and wait for it; leave no process behind.
5. The summary gains `base_check_seconds` when the check finished. H1's rule for recording the
   base tree changes to: record when the base check's state was `Passed`.

**Tests.**
- `run.rs` with a scripted provider and an injected state function: `Passed` from turn 1 → runs
  as if no check; `Failed` at turn 3 → `Blocked` at the start of turn 3 with the steps and tail
  in the message, and no model call for turn 3; `Pending` throughout, then `Passed` at the final
  ask → `Verified`; `Pending` then `Failed` at the final ask → `Verified` with the sentence above
  in the final message.
- `task_runner`/dispatch with real temporary repositories and a gate that sleeps for a bounded,
  injected time or waits on a file: the first model call happens before the base check ends; a
  failing base gate gives outcome `blocked`; `.wt/<id>.base` is gone after the run in every
  outcome; no gate process is left after a run that ends while the check is pending.

**Constraints.** No fallbacks. Resumable: nothing new is kept between runs except H1's records.
No sleeps on wall-clock time in tests: use the injected clock and files to order events. Depends
on: H1. Siblings: none.

**Commit message.** `feat(dispatch): check the base commit beside the model's first turns`
