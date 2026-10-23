# Plan: review against the run's own base; a time-warning test that holds under load (2026-10-23)

## Why

1. On 2026-10-22, `blyte review UN1` and `blyte review CL1` flagged another session's commits and files
   as "not named in the task" and "commit message not in the task". The cause is `run_review`
   (`crates/blyte-cli/src/dispatch.rs`, about lines 1085-1132): it takes the review base as
   `git merge-base <worktree HEAD> <main checkout HEAD>`. The main checkout was on another session's branch,
   while the runs were dispatched with `--base plan/update-notice`. Several sessions share one checkout
   here, so this happens often.
   - The same block falls back to the checkout's HEAD SHA, then to the repository's root commit. Those
     are stand-ins for "unknown", which the no-fallbacks rule forbids.
   - Today `.wt/<id>.start.json` records only `"base": args.base`, which is symbolic (`"HEAD"`).
2. `test_time_left::time_warning_arrives_once_under_ten_percent_of_max_duration`
   (`crates/blyte-agent/tests/test_time_left.rs`) failed 3 of 3 runs locally at load average about 8.
   It fails on main as well, and passes in CI. The test uses a real 12 s `max_duration` with a 250 ms
   provider sleep, then expects exactly one request to carry the time warning (`left: 0` under load).
   Every dispatch in this repository runs the full test suite as its gate, so under load every run fails.
   #302 (`18003d2`) fixed the same kind of test in `test_gate_slot_deadline.rs`.

### Task RB1: record the base commit at dispatch, and review against it

Files: `crates/blyte-cli/src/dispatch.rs`, only the places named here; never read it whole. Plus a new test,
`crates/blyte-cli/tests/test_review_base.rs`.

No fallbacks.

1. **Dispatch** (`grep -n "let base_commit" crates/blyte-cli/src/dispatch.rs`, about line 496, and the
   new-worktree path just above it, `grep -n "worktree add"`):
   - Find the commit the new worktree starts from as a full SHA. Use
     `git rev-parse --verify <args.base>^{commit}` in the root, before `git worktree add`.
     The resume path already has `base_commit`.
   - Record it in `start_json` as `"base_sha"` (`grep -n "\"base\": args.base"`, about line 795).
   - If it cannot be found, dispatch already fails at `worktree add`; keep that error.
2. **Review** (`grep -n "fn run_review"`; the base block is at about lines 1085-1132, which begins with
   `let root_head = Command::new("git")`):
   - Read `base_sha` from `.wt/<id>.start.json`. When it is present and the commit exists in the
     worktree (`git cat-file -e <sha>^{commit}`), it is the review base.
   - When it is present but the commit does not exist: print
     `error: run <id> started from <sha>, which is not in this repository` and return 2.
   - When the start record has no `base_sha` (runs dispatched before this change): keep today's
     `merge-base <worktree HEAD> <root HEAD>`. If that fails, print
     `error: cannot find where run <id> started; its start record has no base_sha and the worktree shares no history with <root HEAD>`
     and return 2.
   - Remove the root-HEAD and root-commit fallbacks.
3. **Tests** (`test_review_base.rs`; copy the repository and run setup from `crates/blyte-cli/tests/test_review_verdict.rs`):
   - **Other branch checked out:** a run `A1` is dispatched from branch `feat/a`, with `base_sha` = the tip
     of `feat/a`, and one commit `feat: a1` on `wip/A1`. The main checkout is on branch `other`, which has
     its own commit `other: unrelated`. `blyte review A1` lists `feat: a1` under `== commits`. It does
     not list `other: unrelated`, and it has no `commit message not in the task: other: unrelated` flag.
   - **Old run:** a start record without `base_sha` still reviews as before, with the checkout on the
     same branch the run started from.
   - **Missing commit:** a `base_sha` that is not in the repository makes the review exit 2 with
     `which is not in this repository`.

Gate: the repository checks. Commit: `fix(review): review a run against the commit it started from, not the checkout's HEAD`

### Task TL1: make the time-warning test independent of machine speed (parallel with RB1)

Files: `crates/blyte-agent/tests/test_time_left.rs`. If, and only if, the agent needs a seam for this,
`crates/blyte-agent/src/run.rs` in the place that decides the time warning
(`grep -n "minutes left" crates/blyte-agent/src/run.rs`). Read about 40 lines around it, and never read
`run.rs` whole.

1. First read how #302 fixed `test_gate_slot_deadline.rs` (`git show 18003d2 --stat`, then that diff for the
   test file only), and use the same approach if it fits.
2. Find why no request carries the warning under load: for example, a slow turn jumps past the
   last 10% window, or the deadline passes before a turn starts there. Change the test so it holds
   whatever the machine speed. For example:
   - drive time from the provider's call count instead of wall-clock sleeps;
   - or use a fake clock, if the runner already has a seam for one.
   The test must still check what it checks today: the warning arrives exactly once, at the top of a
   turn, and appears once in history.
   - Do not loosen the assertions to "at most one".
   - Do not add retries.
3. Prove it: run `cargo test -p blyte-agent --test test_time_left` 5 times while
   `yes > /dev/null` runs on every core (`for i in $(seq $(sysctl -n hw.ncpu 2>/dev/null || nproc)); do yes > /dev/null & done`).
   Kill those processes afterwards (`pkill yes`). Report the 5 results in the commit message body.

Gate: the repository checks. Commit: `test(agent): make the time-warning test independent of machine speed`
