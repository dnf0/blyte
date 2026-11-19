# Plan: environment guard

Spec: `docs/specs/2026-11-19-environment-guard.md`. Three independent tasks, dispatched in
parallel. Docs (`docs/usage.md`, `docs/reference/cli.md`, the bundled skill) are written at merge.

Rules for every task: no fallbacks (a missing value or failed step is an error that names what is
missing, never a default, a guess or a silent skip). No job here runs long, so "resumable" does
not apply. Do not add any name of a private repository to this public repository. Existing tests
pass; change an existing test only where the task says so. The gate is
`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.

### Task EG1: run the gate before the model starts

**Goal.** Before the first model call, a dispatch with a gate runs the gate once on the untouched
worktree. If it fails, the run ends as `blocked` with zero turns. Why: today a worktree whose
gate cannot run (for example, dependencies cannot be installed) is handed to the model, which
burns its budget fighting the environment.

**Depends on:** none. **Siblings:** EG2 (review flag and a prompt rule, in
`crates/blyte-cli/src/dispatch.rs` near `DISPATCH_RULES` and the review code) and EG3 (keep-awake
and a `gcloud` timeout) run separately and are not in this worktree; do not add them.

**Files.** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/cli.rs`,
`crates/blyte-cli/src/config.rs`, `crates/blyte-cli/src/dispatch.rs`,
`crates/blyte-cli/src/task_runner.rs` (and whatever carries a run option from the CLI to the
agent, as `--max-turns` is carried at `dispatch.rs:954`); new tests
`crates/blyte-agent/tests/test_base_check.rs` and `crates/blyte-cli/tests/test_dispatch_base_check.rs`.

**What to build.**
- The agent gets an option `base_check: bool` (default `false` in the library, so existing agent
  tests are unchanged). When it is `true` and the gate has at least one step, the run executes the
  gate with the existing `run_gate` machinery (same sandbox, same environment, same gate slots)
  before the first `provider.complete` call.
  - Gate passes: the run continues exactly as today. The base gate run does not count in
    `gate_attempts`.
  - Gate fails: return `Outcome::Blocked` with the message
    `the check fails before any change (<failing steps joined by "; ">): <last 20 lines of the gate output>. Fix the environment or the base commit, or pass --no-base-check if this task is meant to fix that check.`
    The provider is never called; the report shows 0 turns.
  - With no gate steps, nothing is run.
- The inner runner takes a `--base-check` flag that sets the option.
- `blyte dispatch` passes `--base-check` for every non-research run unless `--no-base-check` is
  given or `blyte.toml` has `base_check = false` under `[dispatch]`
  (`DispatchConfig.base_check: Option<bool>`; absent means on). Research runs never pass it.
- The existing `blocked` handling (exit code 5, summary, `final_message`) is reused; the
  summary's `error` is the first line of the message.
- The start record (`start_json` in `dispatch.rs`) gains `"base_check": true|false`.

**Tests.**
- Agent (`test_base_check.rs`, modelled on `crates/blyte-agent/tests/test_blocked.rs`):
  1. `base_check` on, gate `false`: outcome is `Blocked`, the message contains
     `the check fails before any change` and the step `false`, and the scripted provider received
     zero calls.
  2. `base_check` on, gate `true`: the provider is called and the run ends as it does without
     the option; `gate_attempts` equals the value without the option.
  3. `base_check` off, gate `false`: the provider is called (today's behaviour).
  4. `base_check` on, empty gate: the provider is called.
- CLI (`test_dispatch_base_check.rs`, modelled on `crates/blyte-cli/tests/test_dispatch_setup.rs`):
  5. dispatch with `--gate 'exit 3'`: summary `outcome` is `blocked`, `turns` is 0, process exit
     code is 5, `final_message` contains `--no-base-check`.
  6. the same with `--no-base-check`: the outcome is not `blocked`.
  7. `blyte.toml` with `[dispatch]` `base_check = false` and a failing gate: not `blocked`.
- Existing CLI tests that dispatch with a gate that fails at base and expect another outcome get
  `--no-base-check` added; change nothing else in them.

Run: `cargo test -p blyte-agent --test test_base_check && cargo test -p blyte-cli --test test_dispatch_base_check`.

**Commit message:** `feat(dispatch): run the gate before the model starts and stop as blocked if it fails`

### Task EG2: flag environments borrowed from outside the worktree

**Goal.** Tell the model not to borrow dependency environments from outside the worktree, and
make `blyte review` flag a run that did. Why: two runs reached `verified` on a gate that ran in a
`.venv` copied from elsewhere, and the existing flags did not say that.

**Depends on:** none. **Siblings:** EG1 (a gate run before the model starts) and EG3 (keep-awake,
`gcloud` timeout) run separately and are not in this worktree; do not add them.

**Files.** New `crates/blyte-cli/src/outside_env.rs` (registered in `crates/blyte-cli/src/lib.rs`);
`crates/blyte-cli/src/dispatch.rs` (one rule in `DISPATCH_RULES`, and the call from the review's
checks); new `crates/blyte-cli/tests/test_outside_env.rs`.

**What to build.**
- In `DISPATCH_RULES`, after the "Never delete, clean or modify anything outside the worktree"
  rule, add exactly:
  `- Never copy, link or use a dependency environment from outside the worktree (a .venv, node_modules, site-packages or a package-cache entry from the main checkout, another worktree, another repository or your home directory). If the environment cannot be built here, stop and say so.`
- `outside_env.rs`:
  - `pub const ENV_DIRS: &[&str] = &[".venv", "venv", "node_modules", "site-packages", "archive-v0"];`
  - `pub fn borrowed_env_commands(log: &str, worktree: &Path) -> Vec<String>`: the logged shell
    commands (found the same way `host_commands` in `dispatch.rs` finds them; reuse its command
    extraction, making it `pub(crate)` if needed) that take an environment from outside the
    worktree. A command, split on `&&`, `;` and `|`, counts when one part:
    - starts with `cp`, `ln`, `rsync` or `mv` (after any leading `sudo` or env assignments),
    - has an argument containing a path component in `ENV_DIRS`, and
    - that argument, resolved against the worktree (absolute as is; relative joined to the
      worktree; `~/` and `$HOME/` taken as outside; `..` and `.` normalised without touching the
      file system), is not under the worktree.
    Each command is returned once, in log order, trimmed to 200 characters.
  - `pub fn flag(commands: &[String]) -> Option<String>`: `None` when empty, otherwise
    `the gate may have run in an environment from outside the worktree: <n> command(s), first: <first command>`.
- `blyte review` adds that flag to `== checks` for non-research runs, using the run's worktree
  path from its saved summary (not a guess from the id). If the summary has no worktree path,
  that is an error naming the missing field, not a skip.

**Tests** (`test_outside_env.rs`; worktree `/r/.wt/T1`):
1. `cp -cR /r/.venv .venv` is returned (main checkout).
2. `rm -rf .venv && cp -cR ../T2/.venv .venv` is returned (sibling worktree, relative).
3. `cp -R ~/.cache/uv/archive-v0/abc/lib .venv/lib/python3.12/site-packages/x` is returned.
4. `ln -s /other/repo/node_modules node_modules` is returned.
5. `cp -R .venv/lib/a .venv/lib/b` is not returned (inside the worktree).
6. `cp ../T2/src/a.py src/a.py` is not returned (no environment directory).
7. `ls /r/.venv` and `cat ../T2/.venv/pyvenv.cfg` are not returned (not a copy or link).
8. The same command logged twice is returned once.
9. `flag(&[])` is `None`; `flag` of two commands contains `2 command(s)` and the first command.
10. `DISPATCH_RULES` contains `Never copy, link or use a dependency environment from outside the worktree`.

Run: `cargo test -p blyte-cli --test test_outside_env`.

**Commit message:** `feat(review): flag a gate that may have run in an environment from outside the worktree`

### Task EG3: keep the Mac awake and bound the gcloud token fetch

**Goal.** Keep the machine awake while a dispatch runs on macOS, and give the Vertex token fetch
a timeout. Why: a run stood still for 47 minutes because the Mac went to idle sleep mid-call; and
`gcloud` is called with no time limit.

**Depends on:** none. **Siblings:** EG1 (a gate run before the model starts) and EG2 (a review
flag and a prompt rule) run separately and are not in this worktree; do not add them.

**Files.** New `crates/blyte-cli/src/keep_awake.rs` (registered in `crates/blyte-cli/src/lib.rs`);
`crates/blyte-cli/src/dispatch.rs` (one call after the run's child process is spawned, where
`child.id()` is used for `start_json`); `crates/blyte-provider/src/gemini.rs`
(`GcloudTokenSource::token`); tests in `crates/blyte-cli/tests/test_keep_awake.rs` and
`crates/blyte-provider/tests/` (a new file `test_gcloud_timeout.rs`).

**What to build.**
- `keep_awake.rs`:
  - `pub fn command(pid: u32) -> Option<std::process::Command>`: on macOS,
    `/usr/bin/caffeinate -i -w <pid>`; on other systems `None` (there is nothing to hold; this
    is the intended value, not a stand-in).
  - `pub fn hold(pid: u32) -> Result<(), String>`: spawns that command detached (stdin, stdout,
    stderr null) and does not wait for it; `caffeinate -w` exits by itself when the run's process
    exits. A spawn failure returns `Err("could not start /usr/bin/caffeinate: <error>")`.
  - Setting `BLYTE_NO_KEEP_AWAKE=1` makes `hold` do nothing and return `Ok(())` (for tests and CI).
- `dispatch.rs`: call `keep_awake::hold(child.id())` right after the child is spawned. On `Err`,
  print `blyte: warning: <message>; the machine may sleep during this run` to stderr and carry on
  (the run itself is not affected).
- `gemini.rs`: `GcloudTokenSource::token` runs `gcloud` with a 60-second limit: spawn the child,
  wait up to 60 s (poll `try_wait`), and on expiry kill it and return
  `ProviderError::Auth("`gcloud auth application-default print-access-token` did not finish in 60s; run it by hand to see why")`.
  Put the wait in a helper `fn output_with_timeout(cmd: &mut Command, limit: Duration) -> Result<Output, TimedOut-or-io error>`
  that is `pub` (doc-hidden is fine) so the test can call it with another program. Stdout and
  stderr must be read without deadlock (read them on threads, or after the child exits for small
  output). Behaviour when `gcloud` finishes in time is unchanged, including its error messages.

**Tests.**
1. `keep_awake::command(123)` on macOS has program `/usr/bin/caffeinate` and args
   `["-i", "-w", "123"]`; on other systems it is `None` (`#[cfg]` on the assertions).
2. With `BLYTE_NO_KEEP_AWAKE=1`, `hold` returns `Ok(())` and spawns nothing (assert `Ok`).
3. `output_with_timeout` on `sh -c 'sleep 5'` with a 200 ms limit returns the timeout error in
   under 2 s, and the child is no longer running.
4. `output_with_timeout` on `sh -c 'echo hi'` with a 5 s limit returns stdout `hi\n` and success.

Run: `cargo test -p blyte-cli --test test_keep_awake && cargo test -p blyte-provider --test test_gcloud_timeout`.

**Commit message:** `fix(dispatch): keep the Mac awake during a run and bound the gcloud token fetch`
