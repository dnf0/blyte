# Plan: why runs end in the watchdog; a quiet-clean test that holds under load (2026-11-02)

## Why

1. 48 of 323 run reports on this machine (2026-10-19..23, all repositories) have outcome `error`. Almost all
   of them say `stopped 30 s past --max-seconds N during turn T: a model call or command never returned`, or
   `stopped 30 s past --max-seconds N: gate attempt K was still running`. When the hard watchdog kills a run,
   it ends as an error, not with a clean out-of-time stop. Before planning a fix we need to know what was in
   flight each time, and whether the runner lets a model call, a command or a gate run go past the deadline.
2. `test_clean_cherry_pick_record_squash_commit_quiet_clean` (`crates/blyte-cli/tests/test_clean.rs:1146`)
   failed in a gate run on 2026-10-23 under load. `clean_repo` (`crates/blyte-cli/src/clean.rs`, about
   line 1579) stops a quiet clean after 1 s of wall-clock time, so a slow machine removes fewer worktrees
   than the test expects. Every dispatch here runs the full suite as its gate, so the flake blocks runs.
   #302 (`18003d2`) and #312 fixed tests of this kind.

### Task RW1 (research): what is in flight when the watchdog stops a run

Read-only. The inputs are in `/private/tmp/claude-501/-Users-danielfisher-repos-blyte/1ce1a04e-5c30-4e19-99a7-ecdc94597b60/scratchpad/watchdog-runs/`.
There are 48 runs; each is named `<repo>--<id>` and has `.report.json`, `.log` and, where present,
`.start.json`, `.json` (summary) and `.verdict.json`. The code is this repository (`crates/blyte-agent/src/run.rs`,
`crates/blyte-cli/src/dispatch.rs`, the provider crates). Do not read `run.rs` or `dispatch.rs` whole: grep first.

Answer, with evidence (log lines quoted with file and line, code cited as `path:line`):
1. For each of the 48 runs, what was running when the watchdog fired: a model call (which provider and
   model; how long since the request was sent), a bash command (the command; its timeout), or the gate
   (which attempt; how long it had run). Give a table: run, max-seconds, turn, kind, item, seconds in flight.
   Script the table from the logs; do not judge rows one by one.
2. Totals by kind, and how much run time was spent past the soft deadline.
3. In the code: how the deadline is applied to (a) model calls, including the provider's request and idle
   timeouts, (b) bash command timeouts, (c) gate runs. Say whether each is limited to the time left before the
   deadline, and where the watchdog's 30 s grace is set.
4. Which of these runs left commits or a usable state in their worktree (from the summary and verdict files),
   which tells us what the error outcome loses.
5. A recommended fix, the smallest that would turn these runs into clean out-of-time stops with a report,
   and the files it would touch. No fallbacks in the proposal.

Do not change anything. Findings in `report.md`.

### Task QC1: make the quiet-clean tests independent of machine speed (parallel with RW1)

Files: `crates/blyte-cli/src/clean.rs` (only `clean_repo` and the code that calls it; grep, never read it whole)
and `crates/blyte-cli/tests/test_clean.rs` (the tests that run a quiet clean; `grep -n quiet`).

No fallbacks.

1. Read how #302 fixed `test_gate_slot_deadline.rs` (`git show 18003d2 --stat`, then the test file's diff).
2. Make the 1 s quiet-clean budget something a test can set, so the tests don't depend on wall-clock speed.
   For example, a `time_budget: Option<Duration>` parameter passed down from `clean_quiet`, or an environment
   variable that the tests set to a large value.
   - The production budget stays 1 s.
   - If you add an environment variable, a value that does not parse is an error naming the variable and the
     value, not a silent return to 1 s.
   - Keep one test that proves the budget stops a quiet clean: it should use a budget of zero and expect no
     worktrees removed, rather than relying on a slow machine.
3. Prove it: run `cargo test -p blyte --test test_clean` 5 times while `yes > /dev/null` runs on every core
   (`for i in $(seq $(sysctl -n hw.ncpu 2>/dev/null || nproc)); do yes > /dev/null & done`), then `pkill yes`.
   Report the 5 results in the commit message body.

Gate: the repository checks. Commit: `test(clean): make the quiet-clean tests independent of machine speed`
