# A thin orchestrator: plan

Implements `docs/specs/2026-11-21-thin-orchestrator.md`.

## Facts checked against the code (research run `Rthin`, 2026-11-21)

- Nothing records which request a run was dispatched for. `.wt/<id>.start.json` has `session` (the
  whole surface session), optional `plan`/`plan_task`, `tags` and `base`.
- The only relation between runs outside a plan file is `base: wip/<parent>`.
- No code cherry-picks, merges, pushes or opens a pull request. The gate is a private method,
  `Runner::run_gate` in `crates/blyte-agent/src/run.rs`.
- `model_review::run` exists and is called only by `blyte review <id> --model-review`.
- The surface already has a review queue: `queue_actions::decide` with `Accept`, whose effect today
  is a message to the orchestrator ("Combine it onto the working branch and run the gate").
- blyte's code can already tell the orchestrator something: `Session::submit` queues a message.

Every task: **no fallbacks** (a missing run, branch, remote or tool is an error that names it,
never a skip or a default), and the existing tests pass unmodified unless the task says otherwise.

### Task T1: record which request a run belongs to

**Goal.** Every run dispatched from the surface records the request (the user message) it was
dispatched for, so the runs of one request can be found. Why: combining works per request.

**What must be true when it is done.**
- Each user message the surface hands to the orchestrator has a request id, unique within the
  repository and safe as a branch-name component (for example `r<session start>-<message seq>`).
- `blyte dispatch` takes `--request <id>` and writes it to `.wt/<id>.start.json` as `request`
  (`null` when not given). The value is validated (letters, digits, `-`, `_`, at most 64 chars);
  anything else is an error.
- The host-side `blyte` tool adds `--request <current request id>` to every `dispatch` the
  orchestrator makes on the surface, without the model supplying it, and rejects a `--request` the
  model supplies itself.
- `request` is exposed on the run records the surface serves (`/api/runs`).

**Tests** for: the flag is written and validated; a surface dispatch carries the current request's
id; two messages give two ids; a model-supplied `--request` is refused.

**Depends on:** none. **Siblings:** T3, T5, T6 run beside this and are not in this worktree.

**Commit message:** `feat(dispatch): record the request a run was dispatched for`

### Task T2: `blyte combine <request>`

**Goal.** One command, with no model, puts the passed runs of a request on a work branch and runs
the repository's check on it. Why: the orchestrator did this by hand, at the cost of many turns.

**What must be true when it is done.**
- `blyte combine <request>` finds the runs whose `start.json` has that `request`. None found: an
  error. Any run not finished, not `verified`, or with verdict `redone`: an error naming each such
  run and its state. Research runs are left out.
- Order: a run whose `base` is `wip/<other>` comes after `<other>`; otherwise by start time.
- It works in a worktree of its own (`.wt/combine-<request>`), never in the user's checkout. It
  creates the branch `blyte/<request>` from the commit the group's runs started from (the base of
  the runs that are not based on another run; if those differ, an error naming them), then applies
  each run's own commits with `git cherry-pick -x` (for a run based on another, only the commits
  after its parent's tip).
- It then runs the gate from `blyte.toml`, with the same steps and environment a dispatch uses.
  Factor the gate's step runner out of `Runner` as needed so both use one implementation.
- It writes `.wt/combined/<request>.json`: `state` (`combined`, `conflict`, `check_failed`),
  `branch`, the run ids and tip commits it used, the combined commits, and on failure the
  conflicting run and files or the failing step and the tail of its output. Exit 0 only for
  `combined`.
- On a conflict it aborts the cherry-pick and leaves the branch at the last run that applied. It
  does not try to resolve anything.
- Idempotent and resumable: run again with the same run tips and a `combined` record, it does
  nothing and exits 0; with different tips, or after a failure, it rebuilds the branch from the
  start. It refuses to overwrite a branch `blyte/<request>` it did not create (no record file).

**Tests** (real git repositories in temp directories) for: two independent runs combine and the
gate runs on the result; a run based on another applies only its own commits; a conflict gives
`conflict` and names the run; a failing gate gives `check_failed` with the step; an unfinished or
unverified run is refused by name; a second call is a no-op; an existing foreign branch is refused.

**Depends on:** T1.

**Commit message:** `feat(combine): combine a request's passed runs on a work branch and run the check`

### Task T3: push a branch and open a pull request, as functions

**Goal.** Two functions blyte's code can call: push a work branch, and open a pull request for it.
Why: these become buttons the user presses; the model never runs them.

**What must be true when it is done.**
- A new module `crates/blyte-cli/src/publish.rs` with `push_branch(root, branch)` and
  `open_pull_request(root, branch, title, body)`, each returning a result that names what failed.
- `push_branch` runs `git push -u origin <branch>`, never with force. It refuses, before running
  anything: the repository's default branch (read from `origin/HEAD`; if that cannot be read, an
  error, not a guess), a branch that does not exist, and a repository with no `origin` remote.
- `open_pull_request` runs `gh pr create` with the given title and body against the default
  branch, as arguments (never through a shell), and returns the pull request's URL. `gh` missing or
  not signed in is an error that says so. It refuses an empty title.
- Neither function merges anything, and there is no function here that does.
- No routes and no page changes in this task.

**Tests** for: pushing to a local bare `origin` works; the default branch is refused; a missing
branch, a missing remote and an unreadable default branch are errors; `gh` is run with the expected
arguments (use a stand-in `gh` on `PATH`) and its URL is returned; a missing `gh` is an error.

**Depends on:** none. **Siblings:** T1, T5, T6 are not in this worktree.

**Commit message:** `feat(publish): push a work branch and open a pull request, never the default branch`

### Task T4: combining and the two buttons on the surface

**Goal.** The surface combines a request's runs by itself when they have all passed, shows the
result, and lets the user push and open a pull request. Why: the spec's "Combining" and "Leaving
the machine".

**What must be true when it is done.**
- When a run finishes, if every run of its request is `verified` and none is waiting in the review
  queue (flagged checks with no verdict, or on hold), the server runs T2's combine for that request
  on a background thread. `Accept` in the review queue triggers the same check, and no longer sends
  the "Combine it onto the working branch" message to the orchestrator.
- The surface shows each request's combine state: combining, combined (branch, commits, check
  passed), conflict or check failed (which run, which step, the output tail).
- On `conflict` or `check_failed` the orchestrator is sent one message saying which request and
  why (through the session's queue), once per failure.
- For a `combined` request the surface shows **Push branch** and **Open pull request** (with an
  editable title and body, prefilled from the runs' commit messages). They call
  `POST /api/requests/<id>/push` and `POST /api/requests/<id>/pull-request`, which call T3's
  functions. Both require the per-start secret like every `POST`. The result (pushed; the pull
  request's URL; or the error) is shown and written to the session log.
- A request that is not `combined` cannot be pushed: `409`.
- A finished run with no `.wt/<id>.checks.json` (T5's post-dispatch check could not be computed or
  written) counts as waiting in the review queue, never as clean: its request is not combined.

**Tests** for: the trigger conditions (all passed; one flagged; one on hold; one failed); the
failure message reaches the orchestrator once; the two routes refuse without the secret, refuse a
request that is not combined, and call the functions otherwise; the page renders each state.

**Depends on:** T2, T3, S4a (`docs/plans/2026-11-21-surface-approval.md`).

**Commit message:** `feat(surface): combine a request's runs automatically; push and open a pull request by button`

### Task T5: a fresh review when the checks flag something

**Goal.** A run whose built-in checks flag something, or whose diff touches a risky area, gets a
review from a fresh model run that sees only the task and the diff; a run that flags nothing gets
none. Why: the orchestrator reviewed with the whole conversation loaded, at high cost.

**What must be true when it is done.**
- When a dispatched run finishes `verified`, blyte computes its checks (as `blyte review` does). If
  they flag anything other than "files not named in the task" when the task names no files, or the
  diff touches a risky path, it runs `model_review::run` and stores the result with the checks.
- Risky paths come from `blyte.toml` (`[review] risky = ["glob", ...]`). The key is optional and
  its documented default is an empty list: no path is risky unless the repository says so.
- A run reviewed this way with findings waits in the review queue for the user; it is not combined
  until they accept it.
- A failure of the review run itself is recorded as such and the run waits in the queue; it is not
  treated as "no findings".

**Tests** for: a clean run starts no review; a flagged run does; a risky path does; findings put
the run in the queue; a failed review is recorded and the run waits.

**Depends on:** none. **Siblings:** T1, T3, T6 are not in this worktree.

**Commit message:** `feat(review): review a flagged run with a fresh model run, and only then`

### Task T5b: an unreadable stored review is an error

**Goal.** Found in review of T5: `read_stored_model_review` in
`crates/blyte-cli/src/queue_actions.rs` returns `None` both when a run has no stored review and
when `.wt/<id>.checks.json` cannot be read, is not valid JSON, or holds a `model_review` that does
not deserialize. The second group makes a run with findings look like a run with no review. No
fallbacks: only a missing file, or a missing or `null` `model_review`, means "no review".

**What must be true when it is done.**
- The function returns a result: `Ok(None)` for a missing file or a missing or `null`
  `model_review`; `Ok(Some(..))` for a stored review; an error naming the file and the cause for
  anything else.
- Every caller handles the error: `blyte review <id>` prints it and exits non-zero; anything that
  decides whether a run waits in the review queue treats the run as waiting and shows the error.

**Tests** for: a missing file; a `null` review; a stored review; invalid JSON; a `model_review` of
the wrong shape; `blyte review` exiting non-zero with the file named.

**Depends on:** T5.

**Commit message:** `fix(review): an unreadable stored review is an error, not "no review"`

### Task T6: tests that fail when the machine is busy (split)

The first attempt (one task for five tests in three crates, each to be repeated 20 times under a
concurrent build) ran out of budget with no changes. It is split by crate below. Two more tests
failed the same way later that day and are included: `test_gcloud_timeout` and
`test_gate_slot_deadline`.

The same rules hold for T6a, T6b and T6c:
- The cause is found and stated in the commit body. Where a test waits on real time with a margin,
  it is changed to wait on the event it means (a signal, a file, a channel, an injected clock), or
  its margin is tied to something that scales with load.
- No test is ignored, retried, deleted or loosened so that it no longer checks its behaviour.
- Run each changed test three times. Do not start a concurrent build or a long repeat loop: the
  reviewer repeats the tests under load afterwards.
- Change only the files of the tests named and the code they exercise.

### Task T6a: the provider's timeout tests

**Goal.** `test_model_timeouts` and `test_gcloud_timeout` (test
`output_with_timeout_times_out_and_kills_child`) in `crates/blyte-provider` failed on untouched
code while other builds were running. Make them independent of how fast the machine is, under the
rules above.

**Depends on:** none. **Siblings:** T6b (blyte-agent and blyte-cli deadline tests) and T6c
(`test_update.rs`) are not in this worktree; do not touch those tests.

**Commit message:** `test(provider): make the timeout tests independent of machine load`

### Task T6b: the deadline and watchdog tests

**Goal.** `test_watchdog_progress`, `test_deadline_stop`, `test_deadline_watchdog` and
`test_gate_slot_deadline` (test
`a_gate_waiting_for_a_slot_past_max_duration_still_runs_and_verifies`), in `crates/blyte-agent` and
`crates/blyte-cli`, failed on untouched code while other builds were running. Make them independent
of how fast the machine is, under the rules above.

**Depends on:** none. **Siblings:** T6a (blyte-provider) and T6c (`test_update.rs`) are not in this
worktree; do not touch those tests.

**Commit message:** `test: make the deadline and watchdog tests independent of machine load`

### Task T6c: the update test

**Goal.** `an_edited_skill_is_not_touched` in `crates/blyte-cli/tests/test_update.rs` failed on
untouched code while other builds were running. Make it independent of how fast the machine is,
under the rules above.

**Depends on:** none. **Siblings:** T6a (blyte-provider) and T6b (deadline and watchdog tests) are
not in this worktree; do not touch those tests.

**Commit message:** `test(update): make the edited-skill test independent of machine load`

### Done directly (text only)

- **D1.** The implementer's rules (`DISPATCH_RULES`): say in the report when the request was
  already satisfied, or smaller or larger than stated, and what was done instead. After T1 merges.
- **D2.** The orchestrator's default process: it does not re-run a check a run's gate passed, does
  not read diffs, does not cherry-pick, push or open pull requests; it answers, dispatches, and
  acts on what blyte tells it. After T4 merges.

### Measurement

After T4 and D2: the trial in the spec's "Measurement" section.

## After the first measurement (2026-11-22)

Two things the measurement in `docs/evidence.md` ("A request with several tasks") showed.

### Task F1: the scope check applies only to a task that lists its files

**Goal.** `blyte review`'s check "files not named in the task" exists for a task that lists the
files it may change. A request passed on as it is has no such list; it mentions a file or two in
passing, and the check then flags every other file the run touched. In the measurement this held
every run in the review queue and started a fresh review for each. Apply the check only when the
task has a files section.

**Files.** `crates/blyte-cli/src/review_checks.rs` (the check and its unit tests); any test under
`crates/blyte-cli/tests/` that asserts the old behaviour; `docs/` or
`crates/blyte-cli/assets/orchestrator/reference.md` only where the check is described.

**What to build.**
- `pub fn task_lists_files(task: &str) -> bool`: true when some line of the task, after removing
  leading whitespace and any leading `#`, `*`, `-`, `_` and spaces, starts with `Files`
  (case-insensitive) and the next character is the end of the line, `:`, `*`, `_` or a space.
  So `## Files`, `**Files.** a.rs`, `- **Files**: a.rs`, `Files to change:` and `files:` count;
  `Filesystem notes`, `The files are…` and a file name in a sentence do not.
- In `build`: add the "files not named in the task" flag only when `task_lists_files(task)`.
  Everything else in `build` is unchanged (commit subjects, possible fallbacks, outcome, host
  commands, the "task file not found" flag).
- `should_run_model_review`: replace its use of `task_names_files` with `task_lists_files`. Remove
  `task_names_files` if nothing else uses it.
- Where the check is described for users or the orchestrator, say that it applies to a task with a
  files section.

**Tests** (unit tests in `review_checks.rs`):
- `task_lists_files` for each example above, true and false;
- a task with no files section that mentions `src/a.rs` in a sentence, with `src/a.rs` and
  `src/b.rs` changed: no "files not named" flag, and `should_run_model_review` is false when there
  are no other flags and no risky matches;
- the same task with a `## Files` section listing `src/a.rs`: the flag names `src/b.rs` only, and
  `should_run_model_review` is true;
- a task with no files section and a "possible fallbacks added" flag: still flagged, still reviewed.
Run: `cargo test -p blyte review_checks`, then the gate.

**Constraints.** No fallbacks: this narrows when one check applies; it does not default or hide any
other flag. Existing tests pass unmodified except those asserting the old rule, which are updated
to the new one. Depends on: none. Siblings: F2 changes `requests.rs` separately and is not in this
worktree.

**Commit message.** `fix(review): check a run's files against the task only when the task lists its files`

### Task F2: combine a request only when the orchestrator is idle

**Goal.** blyte combines a request as soon as every run dispatched so far has passed. When the
orchestrator is still in its turn, about to dispatch a run that depends on those, the combine and
its full check run once too early and again later. Wait until the orchestrator has finished.

**Files.** `crates/blyte-cli/src/requests.rs`; `crates/blyte-cli/tests/test_requests.rs`;
`crates/blyte-cli/src/session.rs` only if a test needs a way to put the session into the busy or
queued state; `crates/blyte-cli/assets/orchestrator/reference.md` ("Combining results").

**What to build.**
- In `maybe_combine`, before anything else that starts a combine or queues a failure message: read
  `surface.session.info()`; when `busy` is true or `queued` is greater than 0, return `Ok(false)`.
  The existing callers (the `/api/runs` poll and `Accept`) call it again later, so the combine
  starts on the first call after the orchestrator is idle. Add nothing that polls or sleeps.
- `GET /api/requests`: for a request that is ready but held back for this reason, `state` stays
  `waiting` and `waiting_reasons` is exactly `["the orchestrator is still working"]`.
- `reference.md`, "Combining results": one sentence saying blyte combines once every run has
  passed and the orchestrator's turn has ended, so a run that depends on others should be
  dispatched in the same turn.

**Tests** (`tests/test_requests.rs`, as the existing ones are built):
- a ready request while the session is busy: `maybe_combine` returns `Ok(false)`, no combine
  record or branch is written, and `GET /api/requests` shows `waiting` with that reason;
- the same with a queued message and not busy;
- once the session is idle again, `maybe_combine` combines it (record state `combined`);
- a conflict's failure message is not queued while busy, and is queued exactly once after.
Run: `cargo test -p blyte --test test_requests`, then the gate.

**Constraints.** No fallbacks. Resumable: nothing new is kept in memory; after a restart the
session is idle and the next poll combines. Do not change `combine.rs`, `publish.rs` or
`assets/surface/index.html`. Depends on: none. Siblings: F1 changes `review_checks.rs` separately
and is not in this worktree.

**Commit message.** `fix(surface): combine a request only when the orchestrator's turn has ended`

### Task F3: `/api/runs` fails when a run's worktree disappears while it is being read

**Goal.** On CI (macOS, 2026-11-22) the test
`surface_dispatch_carries_current_request_and_two_messages_get_two_ids` in
`crates/blyte-cli/tests/test_dispatch_request.rs` failed once at its `GET /api/runs` assertion
(expected 200, got 500) with the body:

`run M2 started from <sha>, which is not in this repository (git cat-file in <repo>/.wt/M2: fatal: Unable to read current working directory: No such file or directory)`

It passes on most runs. The error comes from `crates/blyte-cli/src/queue_actions.rs` (about line
184): the code found that the worktree `.wt/M2` existed (`wt_exists`), and by the time it ran
`git cat-file` there the directory was gone. Find what removes `.wt/M2` during that test and make
the outcome the same every time.

**Files.** `crates/blyte-cli/src/queue_actions.rs`; `crates/blyte-cli/src/serve.rs` and the module
that removes finished worktrees (find it: the removal done by `blyte dispatch` and `blyte clean`)
only as the cause requires; `crates/blyte-cli/tests/test_dispatch_request.rs`; a new test file
under `crates/blyte-cli/tests/` if needed.

**What to build.**
- First find the cause and state it in the report with file and line: which code removes the
  worktree, and why it can still be running (or run) when the test's `GET /api/runs` reads it.
- If the removal is legitimate (a finished run's worktree may be removed at any time by another
  `blyte` process), then reading a run must not depend on the worktree still being there between
  two steps: decide once per run whether its worktree is present, and when it is not, take the
  existing path for a run with no worktree (the `else` of `if wt_exists`). A worktree that vanishes
  part-way must lead to that same path, not to a 500 for the whole listing. Commands that only
  need the object database (`git cat-file -e <sha>^{commit}`) run in the repository root, not in
  the worktree.
- If the removal is not legitimate (the test or the code removes a worktree it should keep), fix
  that instead, and say so.
- Do not add sleeps, retries or loosened assertions.

**Tests.**
- A test that fails before the fix every time: build the state the race produces (a run whose
  start file and summary exist and whose worktree directory is removed, by the test, at the point
  the cause identifies; if the cause is a vanished worktree, simply remove `.wt/<id>` after the
  run finishes) and assert `GET /api/runs` returns 200 and lists the run.
- Run `cargo test -p blyte --test test_dispatch_request` 20 times in a row and report the count.
Then the gate.

**Constraints.** No fallbacks: a start commit that is truly not in the repository is still the
error it is today; only "the worktree is gone" changes path. Existing tests pass unmodified.
Depends on: none. Siblings: none.

**Commit message.** `fix(surface): list runs when a finished run's worktree has been removed`
