# Plan: sandbox the orchestrator's shell

Spec: `docs/specs/2026-11-21-orchestrator-sandbox.md`. Base branch for every task:
`feat/orchestrator-instructions` (PR #379), because the reference file these tasks edit is added
there. S1, S2 and S3 change shared files and run one after another. S4 needs the surface (PR #377)
and waits for it to merge.

## Facts checked (research run `R-orch-approval`, 2026-11-21)

- The gate runs inside the implementer's sandbox (`Runner::run_gate`, `crates/blyte-agent/src/run.rs`),
  and this repository's sandbox tests return early when a sandbox cannot be started inside another
  (macOS exit 71). So the full check can run in a sandboxed orchestrator shell.
- Nothing lets a running tool call ask the user a question: not the surface (events go one way;
  input is read only between runs) and not the terminal conversation (`repl.rs` holds the stdin lock
  for the whole run; child commands get no stdin).
- There is no user-level TOML file. `~/.config/blyte/` holds `env`, `orchestrator.md` and
  `dashboard.toml`. The allow list is therefore a plain file, `allow-unsandboxed`, like `env`.
- The `bash` tool takes only `command` (`tools::specs`, `tools::execute_unbounded`), and the model's
  commands go through `Shell::exec_user`.

Rules for every task: no fallbacks (a missing or unreadable input is an error that names it, never a
default or a silent skip). Add no comments other than documentation comments. Existing tests pass
unmodified unless the task names them. The check is the repository's gate.

### Task S1: approval and the unsandboxed flag in the agent crate

**Goal.** Let one `bash` command run outside the sandbox, only when a rule the user wrote allows it
or the user approves its exact text. This task adds the mechanism; nothing turns it on yet.

**Files.** Create `crates/blyte-agent/src/approval.rs` and
`crates/blyte-agent/tests/test_approval.rs`. Change `crates/blyte-agent/src/lib.rs`, `tools.rs`,
`shell.rs`.

**What to build.**

- `approval.rs`:
  - `pub enum Approval { Approved, Denied, Unavailable }`.
  - `pub trait Approver: Send + Sync { fn ask(&self, command: &str, reason: &str) -> Approval; }`
  - `pub struct NoApprover;` whose `ask` returns `Unavailable`.
  - `pub fn is_simple_command(command: &str) -> bool`: false when the trimmed command is empty or
    contains any of `|`, `;`, `&`, `<`, `>`, `$`, a backtick, `(`, `)`, `{`, `}`, a newline or a
    backslash. Otherwise true.
  - `pub fn matching_rule<'a>(command: &str, rules: &'a [String]) -> Option<&'a str>`: `None` unless
    `is_simple_command`. Otherwise the first rule `r` (trimmed, non-empty) for which the trimmed
    command equals `r` or starts with `r` followed by a space.
  - `pub struct ApprovalConfig { pub approver: std::sync::Arc<dyn Approver>, pub rules: Vec<String> }`.
- `tools.rs`: `pub fn set_approval(config: Option<ApprovalConfig>)` and a reader, stored the way
  `set_dispatch_cmd` stores its value.
  - When approval is set, the `bash` tool's schema in `tools::specs()` gains two optional
    properties: `unsandboxed` (boolean; "Run this one command outside the sandbox, with the user's
    network and credentials. It runs only if the user approves it.") and `reason` (string; "One
    line telling the user why the command must leave the sandbox."). When approval is not set, the
    schema is exactly as now.
  - In the `"bash"` arm of `execute_unbounded`, when `unsandboxed` is true:
    - approval not set: error `this run cannot run commands outside the sandbox`;
    - `reason` missing or blank: error `give a one-line reason with an unsandboxed command`;
    - a rule matches: run with `Shell::exec_host`;
    - otherwise call `approver.ask(command, reason)`. `Approved`: run with `exec_host`. `Denied`:
      error `the user refused this command; it was not run. Do not try to run it another way.`
      `Unavailable`: error `nobody can approve a command in this run, so it was not run`.
    - The result of a command that ran starts with one line: `ran outside the sandbox (allowed by
      rule "<rule>")` or `ran outside the sandbox (approved by the user)`.
  - `unsandboxed` absent or false: behaviour unchanged.
- `shell.rs`: add to `trait Shell`
  `fn exec_host(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String>`
  with a default body returning `Err("this shell cannot run a command outside the sandbox".into())`.
  `LocalShell` and `PersistentShell` implement it: a fresh `bash -lc <command>` in `cwd`, with no
  sandbox wrapper, stdin null, the timeout enforced as `exec` enforces it, and the process
  environment with only the harness variables removed (`HARNESS_ENV` and `BLYTE_*` other than
  `BLYTE_SESSION`); credential variables stay, since such a command exists to use them. Container
  shells keep the default.

**Tests** (`cargo test -p blyte-agent --test test_approval`), one each:

- `is_simple_command` is true for `git push -u origin my-branch` and false for each of: an empty
  string, `git push; rm -rf x`, `a && b`, `a | b`, `echo $(x)`, `` echo `x` ``, `a > f`, `a\nb`.
- `matching_rule` returns the rule for `git push -u origin x` with rule `git push -u origin`;
  `None` for `git push -u originx`; `None` for `git push -u origin x; ls` though the prefix
  matches; `None` for a blank rule; an exact match returns the rule.
- With a fake `Shell` that records which method ran, and a fake `Approver` that records its calls:
  no approval set returns the first error and runs nothing; a blank reason returns the second error
  and does not ask; a matching rule runs `exec_host` without asking; `Approved` runs `exec_host`
  once and the output's first line is the approved-by-user line; `Denied` and `Unavailable` return
  their errors and run nothing; `unsandboxed: false` runs `exec_user` and never asks.
- `specs()` has no `unsandboxed` property without approval and has both properties with it.
- `LocalShell::exec_host` with the sandbox on runs `echo hi` and returns `hi` (skip on a platform
  where `sandbox::available()` is false only for the "sandbox on" construction, not the assertion).

Tests that set the global approval must not run in parallel with each other: guard them with one
static mutex and clear the setting at the end of each.

**Depends on:** none. **Siblings:** S2 adds a `blyte` tool; S3 turns the sandbox on. Neither is in
this worktree; do not add them.

**Commit message:** `feat(agent): run a bash command outside the sandbox only with the user's approval`

### Task S2: blyte's own commands as a host-side tool

**Goal.** A sandboxed orchestrator shell cannot run `blyte dispatch` (no key, no network, and macOS
refuses a sandbox inside a sandbox). Give the orchestrator a `blyte` tool the harness runs on the
host, and make the reference describe that one way.

**Files.** Change `crates/blyte-agent/src/tools.rs`, `crates/blyte-cli/src/task_runner.rs`,
`crates/blyte-cli/assets/orchestrator/reference.md`,
`crates/blyte-cli/tests/test_orchestrator_reference.rs` only if its parsing needs it. Create
`crates/blyte-agent/tests/test_blyte_tool.rs`.

**What to build.**

- `tools.rs`: `pub fn set_blyte_tool(exe: Option<std::path::PathBuf>)`, stored like the dispatch
  command. When set, `specs()` offers a tool `blyte` with `args` (array of strings, required: the
  words after `blyte`) and `background` (boolean, optional).
  - `pub const BLYTE_TOOL_COMMANDS: [&str; 7] = ["dispatch", "wait", "status", "review", "plan", "init", "clean"]`.
  - Empty `args`, or a first word not in the list: error
    `the blyte tool runs only: dispatch, wait, status, review, plan, init, clean`.
  - `background: true` with a first word other than `dispatch`: error
    `only dispatch can run in the background`.
  - It runs the stored executable directly with `args` (no shell), in the workspace root, with
    `BLYTE_DISPATCH_PARENT` set as the existing `dispatch` tool sets it and stdin null.
  - Foreground: wait for it within the tool deadline; return stdout, and on a non-zero exit the
    exit code, stdout and the last lines of stderr as the existing `dispatch` tool does.
  - Background: stdout and stderr to null, do not wait, return
    `started <id> in the background; use wait to learn when it ends` where `<id>` is `args[1]`
    (error `dispatch needs an id` if there is none).
- `task_runner.rs`: in an orchestrator run call `set_blyte_tool(Some(std::env::current_exe()?))`;
  an error from `current_exe` stops the start and names the cause. `--dispatch-cmd` and its tool
  are left as they are.
- `reference.md`: rewrite "Dispatching a task" and "While runs are going" so that the one way to
  run blyte's commands is the `blyte` tool. Commands stay written as command lines
  (`blyte dispatch <id> <task.md>`), with one sentence saying the words after `blyte` are passed as
  `args`. Replace the background shell example with two tool calls with `background: true`
  followed by `blyte wait --any T1 T2`. Remove the sentences about redirecting output and about a
  foreground dispatch holding the command. Say that the shell is sandboxed: it can write only in
  the repository and temporary directories, cannot read credentials and has no network, and that a
  command which must leave it (for example `git push`) takes `unsandboxed` with a reason and runs
  only when the user approves. Keep every other fact. Facts only: no advice.

**Tests** (`cargo test -p blyte-agent --test test_blyte_tool` and
`cargo test -p blyte --test test_orchestrator_reference`):

- With the executable set to a small script the test writes (it prints its arguments, its working
  directory and `$BLYTE_DISPATCH_PARENT`): `args: ["status", "T1"]` returns those arguments, the
  root as directory and a non-empty parent.
- `args: []`, `args: ["comments"]` and `args: ["--orchestrate", "x"]` return the list error and the
  script does not run (it would create a marker file).
- `background: true` with `["wait", "T1"]` returns its error; with `["dispatch"]` returns
  `dispatch needs an id`; with `["dispatch", "T1", "t.md"]` returns the started line at once while
  the script sleeps two seconds, and the marker file appears afterwards.
- A script that exits 3 with text on stderr returns an error containing that text.
- `specs()` has no `blyte` tool when unset.
- The existing reference test still passes; `reference.md` contains `unsandboxed` and does not
  contain `> /dev/null`.

**Depends on:** S1 (base `wip/S1`). **Siblings:** S3 turns the sandbox on; not in this worktree.

**Commit message:** `feat(orchestrator): run blyte's own commands through a host-side tool`

### Task S3: turn the sandbox on for the orchestrator

**Goal.** The orchestrator's shell is sandboxed by default, with approval wired for a conversation
at a terminal and refused in a one-shot run.

**Files.** Change `crates/blyte-cli/src/task_runner.rs`, `crates/blyte-cli/src/repl.rs`,
`crates/blyte-cli/src/cli.rs` if a flag's type must change, `crates/blyte-cli/src/instructions_cmd.rs`.
Create `crates/blyte-cli/src/approval_file.rs`, `crates/blyte-cli/src/tty_approver.rs`,
`crates/blyte-cli/tests/test_orchestrator_sandbox.rs`. Change `docs/security.md`.

**What to build.**

- `task_runner.rs`: remove the `if cli.orchestrate { "off" }` branch, so `--sandbox` applies to an
  orchestrator as to any run. When `--orchestrate` is given and `--network` was not passed, the
  network is `off`; an explicit `--network on` is honoured. When the orchestrator runs from a
  linked git worktree, the sandbox also allows writes to the main checkout's `.wt/` and to the
  git common directory.
- `approval_file.rs`: `pub fn load() -> Result<Vec<String>, String>` reads
  `$XDG_CONFIG_HOME/blyte/allow-unsandboxed`, else `~/.config/blyte/allow-unsandboxed`: one command
  prefix per line, blank lines ignored. No file: an empty list (the documented default: nothing is
  allowed in advance). A file that exists but cannot be read: an error naming it.
- `tty_approver.rs`: `pub struct TtyApprover;` implementing `Approver`. `ask` opens `/dev/tty`,
  writes `Run outside the sandbox? <reason>\n  <command>\n[y/N] `, reads one line, and returns
  `Approved` only for `y` or `yes` (any case, trimmed), `Denied` for anything else, and
  `Unavailable` when `/dev/tty` cannot be opened. A function
  `pub fn decide(answer: &str) -> Approval` holds the parsing so it can be tested.
- Wiring, when the run is an orchestrator with the sandbox on: call `tools::set_approval` with the
  loaded rules and `TtyApprover` in a conversation (`repl.rs`), `NoApprover` in a one-shot run.
  With `--sandbox off`, approval is not set.
- The start line, printed where the process and repository rules lines are printed, gains
  `sandbox: on (a command leaves it only with your approval; N allowed in advance)` or
  `sandbox: off`.
- `blyte instructions` prints the `bash` tool's description with the two new properties, as the
  run would see it.
- `docs/security.md`: replace what it says about the orchestrator's shell with the new behaviour,
  in the same plain style.

**Tests** (`cargo test -p blyte --test test_orchestrator_sandbox`):

- `decide` returns `Approved` for `y`, `Y`, ` yes `; `Denied` for `n`, an empty string, `yep`.
- `approval_file::load` with `XDG_CONFIG_HOME` at a temporary directory: no file gives an empty
  list; a file with two prefixes and a blank line gives two; a directory at that path gives an
  error containing the path.
- The task configuration built from `--orchestrate` has the sandbox the flag asked for (default
  on, not `off`) and network `off`; with `--network on`, `on`; with `--sandbox off`, `off`.
- On macOS or Linux where `sandbox::available()`: an orchestrator shell built from that
  configuration fails to write a file in a temporary directory outside the root and succeeds
  inside the root; `git commit` in a repository at the root succeeds.
- `blyte instructions` output contains `unsandboxed`.
- Every test that starts `blyte` sets `HOME` and `XDG_CONFIG_HOME` to a temporary directory and
  removes the provider key variables and `BLYTE_ENV_FILE`, so no test can reach a real key.

**Depends on:** S2 (base `wip/S2`). **Siblings:** S4 adds approval on the surface; not here.

**Commit message:** `feat(orchestrator): sandbox the orchestrator's shell by default`

### Task S4: approval on the surface

Waits for PR #377 to merge; to be written then against the merged code. Outline: a pending
approval entry in the session log (command, reason, an id), a `POST` endpoint that records yes or
no for that id, an `Approver` that blocks the tool call on a condition variable until the answer
arrives or the run is stopped, and the question shown on the surface with the exact command.

### Task S5: tests must not reach a real key

**Goal.** `crates/blyte-cli/tests/test_dispatch_branch_exists.rs` sets `HOME` but not
`XDG_CONFIG_HOME`, so with `XDG_CONFIG_HOME` set and a key in `$XDG_CONFIG_HOME/blyte/env` the test
ran a real dispatch against the model (found 2026-11-21). Make every CLI test that starts `blyte`
immune to the user's configuration.

**Files.** Test files under `crates/blyte-cli/tests/` only.

**How.** Find them with one search (`grep -L XDG_CONFIG_HOME` over the files that call
`CARGO_BIN_EXE_blyte` and set `HOME`), and add `.env("XDG_CONFIG_HOME", <the same directory HOME is
set to>)` beside each `.env("HOME", …)`. Do it with one script, then fix exceptions by hand.

**Tests.** The whole check passes with `XDG_CONFIG_HOME` pointing at a directory that holds
`blyte/env` with the line `GEMINI_API_KEY=invalid-test-key`: run
`XDG_CONFIG_HOME=<dir> cargo test -p blyte --test test_dispatch_branch_exists` and it passes within
ten seconds.

**Depends on:** none (base `main`). **Siblings:** S1 to S3 change other files.

**Commit message:** `test: keep CLI tests away from the user's blyte configuration`
