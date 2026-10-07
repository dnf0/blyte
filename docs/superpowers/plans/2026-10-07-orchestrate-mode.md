# Plan: `blyte --orchestrate`

Spec: `docs/superpowers/specs/2026-10-07-orchestrate-mode.md`. OC1 and OC2 are independent and run in
parallel; OC3 depends on both.

### Task OC1: full replies in interactive output

**Goal.** `format_event` in `crates/blyte-cli/src/repl.rs` cuts a model reply (`Event::Turn`) to 80
characters plus `...`. The user cannot read plans or questions. Show the whole reply.

**Files.** `crates/blyte-cli/src/repl.rs`, `crates/blyte-cli/tests/test_repl_commands.rs`. Nothing else.

**What to build.**
- In `format_event`, the `Event::Turn { n, text }` arm returns `format!("[Turn {}] {}", n, text)`: the
  full text, unchanged, including newlines. Remove the `preview` truncation. No other arm changes.
- `task_runner.rs` also calls `format_event` to write dispatch logs; full text in the log is intended.
  Do not change `task_runner.rs`.

**Tests** (add to `crates/blyte-cli/tests/test_repl_commands.rs`, next to `test_format_event`):
- `test_format_event_turn_full_text`: a `Event::Turn { n: 3, text }` where `text` is 300 characters
  (e.g. `"x".repeat(299) + "Z"`) formats to exactly `format!("[Turn 3] {text}")`; it contains `"Z"` and
  does not end with `"..."`.
- `test_format_event_turn_multiline`: text `"Plan:\n1. a\n2. b\nShall I go ahead?"` formats to exactly
  `"[Turn 1] Plan:\n1. a\n2. b\nShall I go ahead?"`.
- `test_format_event_turn_multibyte`: 100 copies of `"é"` come through whole (`.chars().count()` of the
  output is `"[Turn 1] ".len() + 100`).
- The existing `test_format_event` passes unmodified.

Run: `cargo test -p blyte-cli --test test_repl_commands`.

**Constraints.** Existing tests pass unmodified. No new dependencies.

**Depends on:** none.

**Siblings.** OC2 adds a `blyte wait` command in `wait.rs`/`cli.rs`/`main.rs`/`lib.rs` in a separate
worktree; it is not here, do not add it.

**Commit message:** `fix(repl): show the model's whole reply instead of cutting it at 80 characters`

### Task OC2: `blyte wait`

**Goal.** An orchestrator whose shell commands time out at 300 s needs to wait for dispatches that take
10-30 minutes. Add `blyte wait <ID>... [--any] [--timeout <secs>]`: it blocks until the named runs have
finished (or, with `--any`, until at least one has), then prints one line per finished run.

**Files.**
- Create `crates/blyte-cli/src/wait.rs` (the logic and its unit tests).
- `crates/blyte-cli/src/lib.rs`: add `pub mod wait;` (alphabetical, after `update`).
- `crates/blyte-cli/src/cli.rs`: a `Wait` variant on `Command`.
- `crates/blyte-cli/src/main.rs`: route `Command::Wait` to `blyte_cli::wait::run`.

No documentation change here; OC3 documents `blyte wait` with orchestrate mode.

**What to build.**

CLI (in `Command`, after `Status`):
```rust
    /// Wait until dispatched runs finish (all of them, or the first with --any) and print one line per finished run
    Wait {
        /// Run ids, as given to `blyte dispatch`
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
        /// Return as soon as at least one run has finished
        #[arg(long)]
        any: bool,
        /// Give up after this many seconds (exit 2); stays under the 300 s shell-command limit
        #[arg(long, default_value_t = 280)]
        timeout: u64,
    },
```
`main.rs`: `Command::Wait { ids, any, timeout } => std::process::exit(blyte_cli::wait::run(&ids, any, timeout))`.

`wait.rs`:
- `pub fn run(ids: &[String], any: bool, timeout_secs: u64) -> i32`: finds the repository root with
  `git rev-parse --show-toplevel` exactly as `dispatch::run_status` does (same error messages, return 2),
  then calls `wait_in(&root.join(".wt"), ids, any, Duration::from_secs(timeout_secs), Duration::from_secs(2), &mut std::io::stdout(), &mut std::io::stderr())`.
- `pub fn wait_in(wt: &Path, ids: &[String], any: bool, timeout: Duration, poll: Duration, out: &mut impl Write, err: &mut impl Write) -> i32`.
- A run's state, checked in this order:
  1. **finished**: `wt/<id>.json` exists and parses as JSON. The line is
     `<id> <outcome> <turns> <cost>`: `outcome` the string field (`?` if missing), `turns` the integer
     field (`?` if missing), `cost` as `$` followed by `cost_usd` with 2 decimals (`$0.42`), or `-` if
     missing/null. Example: `OC1 verified 41 $0.37`.
  2. **running**: `wt/<id>.start.json` exists, its `pid` is > 0 and alive (`libc::kill(pid, 0) == 0`, or
     it fails with `EPERM`; same test as `run_status`).
  3. **dead**: otherwise, if `wt/<id>.start.json` exists. Before declaring a run dead, check
     `wt/<id>.json` once more (the run may have written its summary and exited between the two checks);
     if it now exists the run is finished. The line is `<id> dead - -`.
  4. **unknown**: neither `wt/<id>.json` nor `wt/<id>.start.json` exists. Checked once, before waiting:
     write `no dispatch <id> here` to `err` and return 1 without waiting. A `.json` that does not parse
     counts as not existing yet (it may be mid-write).
- Loop: each poll, check every id not yet printed; print the line of each newly finished or dead run
  immediately, in the order the ids were given, and flush `out`. Return 0 when every id is printed, or,
  with `any`, at the end of the first poll in which at least one line was printed (all runs finished by
  then are printed, not only the first). Otherwise sleep `poll` and repeat. Already-finished runs are
  reported on the first poll without sleeping.
- Timeout: when the elapsed time reaches `timeout` with work outstanding, write
  `still running: <id> <id>...` (the unprinted ids, in the given order) to `err` and return 2. A `timeout`
  of 0 means one poll only.
- Duplicate ids are reported once.

**Tests** (unit tests in `wait.rs`, using `tempfile::tempdir()` as the `.wt` directory, `Vec<u8>` for
`out`/`err`, `poll` of 10 ms; check `tempfile` is already a dev-dependency of `blyte-cli`, it is used by
other tests):
- `finished_run_prints_summary_line`: `T1.json` = `{"outcome":"verified","turns":41,"cost_usd":0.374}` →
  returns 0, `out` is exactly `"T1 verified 41 $0.37\n"`.
- `missing_cost_prints_dash`: `{"outcome":"gate_failed","turns":7,"cost_usd":null}` → `"T1 gate_failed 7 -\n"`.
- `dead_run`: `T1.start.json` = `{"pid": <pid of a child process that has been spawned and waited for, e.g. Command::new("true")>}` and no `T1.json` → returns 0, `"T1 dead - -\n"`.
- `zero_pid_is_dead`: `{"pid": 0}` → `"T1 dead - -\n"`.
- `unknown_id_errors_without_waiting`: no files for `T9` → returns 1, `err` contains `no dispatch T9 here`,
  `out` empty, and it returns in well under the timeout (timeout 5 s, assert elapsed < 1 s).
- `timeout_with_running_run`: `T1.start.json` with `std::process::id()` (alive) → with timeout 50 ms returns
  2, `out` empty, `err` contains `still running: T1`.
- `all_mode_waits_for_every_run`: T1 finished, T2 alive (current pid); a thread writes `T2.json` after
  100 ms; timeout 5 s → returns 0, `out` is `"T1 ... \nT2 ...\n"` with T1's line first.
- `any_mode_returns_after_first`: T1 alive, T2 finished → with `any` returns 0 at once, `out` is only T2's
  line.
- `any_mode_prints_all_already_finished`: T1 and T2 both finished, `any` → both lines, in the given order.
- `any_mode_timeout`: T1 alive, `any`, timeout 50 ms → returns 2.
- `summary_written_after_start_counts_as_finished`: both `T1.start.json` (dead pid) and `T1.json` exist →
  the summary line, not `dead`.
- `unparseable_summary_is_not_finished`: `T1.json` = `{"outc` and `T1.start.json` with the current pid,
  timeout 50 ms → returns 2.
- `duplicate_ids_reported_once`: ids `[T1, T1]`, T1 finished → one line.
- `cli_parses_wait`: `Cli::try_parse_from(["blyte", "wait", "A", "B", "--any", "--timeout", "10"])` gives
  `Some(Command::Wait { ids: vec!["A","B"], any: true, timeout: 10 })`; `["blyte","wait","A"]` gives
  `timeout: 280, any: false`; `["blyte","wait"]` is an error.

Run: `cargo test -p blyte-cli wait`.

**Constraints.** Existing tests pass unmodified. No new dependencies (`libc`, `serde_json`, `tempfile`
are already there). Do not change `dispatch.rs`; copying the small liveness check into `wait.rs` is fine.

**Depends on:** none.

**Siblings.** OC1 changes `format_event` in `repl.rs` in a separate worktree; do not touch `repl.rs`.

**Commit message:** `feat(wait): blyte wait blocks until dispatched runs finish and prints one line per run`
