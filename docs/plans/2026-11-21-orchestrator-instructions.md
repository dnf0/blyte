# Plan: the orchestrator's instructions

Spec: `docs/specs/2026-11-21-orchestrator-instructions.md`. Branch: `feat/orchestrator-instructions`.

The three text files are already on the branch and are not to be edited by any task:
`crates/blyte-cli/assets/orchestrator/reference.md`, `reference-dispatch-tool.md`, `process.md`.

Order: O1 and O3 at the same time; O2 after O1. Then the trial, run by the reviewer.

Every task: no fallbacks (fail loudly with an error that names what is missing; never substitute a
default, a guess or a silent skip). Add no comments to code. Run the gate before finishing.

### Task O1: assemble the orchestrator's instructions from a reference, a process and a repository file

**Goal.** The orchestrator is given blyte's reference (facts), then one process (the user's file, or
the built-in default), then the repository's own orchestrator file if it has one. The built-in
preamble, the injected skill, the eight-call dispatch reminder and the implementer's repository rules
are no longer given to it. Every start says which files are in force. Why: the built-in text mixed
facts with a prescribed way of working and contradicted itself; the process is the user's choice.

**Files.**
- `crates/blyte-agent/src/orchestrator.rs` (rewrite)
- `crates/blyte-agent/src/run.rs`
- `crates/blyte-cli/src/orchestrator_text.rs` (new), `crates/blyte-cli/src/lib.rs` (declare it)
- `crates/blyte-cli/src/task_runner.rs`, `crates/blyte-cli/src/repl.rs`
- `crates/blyte-cli/tests/test_orchestrator_instructions.rs` (new)
- existing tests that assert removed text or call removed functions (see Constraints)

**What to build.**

In `crates/blyte-agent/src/orchestrator.rs`, keep `user_rules_path` and `user_rules_path_from` as
they are. Remove `PREAMBLE`, `DISPATCH_TOOL_INSTRUCTION`, `load_user_rules`,
`prompt_sections_with_dispatch` and any other prompt-building function there. Add:

```rust
pub enum Answers { Person, Nobody }
pub struct Part { pub source: String, pub text: String }
pub struct Sources { pub process: Option<Part>, pub repository: Option<Part> }

pub const DEFAULT_PROCESS_SOURCE: &str = "default (built in)";
pub const ANSWERS_PERSON: &str = "A person is reading your replies. A reply that ends with a question and calls no tool waits for their answer, which arrives as the next message.";
pub const ANSWERS_NOBODY: &str = "Nobody will answer during this run. It ends when you reply without calling a tool, so a question ends the run unanswered.";

pub fn repository_rules_path(root: &Path) -> PathBuf;
pub fn load_rules_file(path: &Path) -> Result<Option<String>, String>;
pub fn resolve_sources(user_path: Option<&Path>, root: &Path, default_process: Option<&str>) -> Result<Sources, String>;
pub fn assemble(reference: &str, answers: Answers, sources: &Sources) -> String;
pub fn start_lines(sources: &Sources) -> [String; 2];
```

- `repository_rules_path(root)` is `root/.blyte/orchestrator.md`.
- `load_rules_file`: `Ok(None)` when the path does not exist. `Ok(Some(text))` with the text trimmed
  otherwise. `Err` when the file exists and cannot be read (`cannot read <path>: <io error>`), is
  empty after trimming (`<path> is empty`), or has more than `crate::rules::MAX_RULES_CHARS`
  characters (`<path> has <n> characters; the limit is <MAX_RULES_CHARS>`). Nothing is ever cut.
- `resolve_sources`: the process is the user's file when `user_path` is `Some` and the file exists
  (`source` = the path as displayed); otherwise `default_process` with `source` =
  `DEFAULT_PROCESS_SOURCE`; `None` when there is neither. The repository part is
  `repository_rules_path(root)` when it exists. Any `Err` from `load_rules_file` is returned.
- `assemble` returns exactly these sections in this order, each followed by one blank line; the
  process and repository sections appear only when present:

  ```
  ## blyte reference
  {reference, trimmed}

  {ANSWERS_PERSON or ANSWERS_NOBODY}

  ## Process (from {source})
  {text}

  ## This repository's rules (from {source})
  {text}

  ```
- `start_lines` returns `["process: {source}", "repository rules: {source}"]`, with `none` in place
  of a missing part.

In `crates/blyte-agent/src/run.rs`:
- In `Runner::system_prompt`, the branch taken when `state.orchestrator.instructions` is set no
  longer includes the repository rules (`crate::rules::prompt_section`). Move that branch's fixed
  text after the environment (from `Change nothing outside the repository yourself` to the end of
  `## Working efficiently`) into `pub const ORCHESTRATOR_FIXED: &str` and use it there, text
  unchanged.
- Remove `ORCHESTRATOR_EXPLORE_NUDGE`, the reminder it triggers, and any counter used only for it.

In `crates/blyte-cli/src/orchestrator_text.rs`:

```rust
pub const REFERENCE: &str = include_str!("../assets/orchestrator/reference.md");
pub const REFERENCE_DISPATCH_TOOL: &str = include_str!("../assets/orchestrator/reference-dispatch-tool.md");
pub const DEFAULT_PROCESS: &str = include_str!("../assets/orchestrator/process.md");

pub struct Built { pub instructions: String, pub start_lines: [String; 2] }
pub fn build(dispatch_tool: bool, answers: Answers, root: &Path) -> Result<Built, String>;
```

`build` uses `REFERENCE_DISPATCH_TOOL` and no default process when `dispatch_tool` is true, and
`REFERENCE` with `DEFAULT_PROCESS` otherwise; the user's path is
`blyte_agent::orchestrator::user_rules_path()`.

In `crates/blyte-cli/src/task_runner.rs` and `crates/blyte-cli/src/repl.rs`, every place that sets
`state.orchestrator.instructions` calls `build` (one shared helper, not three copies). `answers` is
`Person` when `state.runtime.conversation` is `Some`, `Nobody` otherwise; `root` is the repository
root the run uses. An `Err` stops the start: it is reported the way the surrounding function already
reports a start error, with the message unchanged, and the run does not begin.

Start lines:
- One-shot runs print both to stderr as `blyte: {line}` before the first model call.
- The interactive banner in `repl.rs` prints them after `  Mode       : orchestrator` as
  `  Process    : {source}` and `  Repo rules : {source}`.
- The existing `blyte: repository rules from <path>` line is not printed for an orchestrator run,
  since those rules are no longer given to it. It is unchanged for implementer runs.

**Tests.** `cargo test -p blyte-agent orchestrator` and
`cargo test -p blyte --test test_orchestrator_instructions`. Unit tests in `orchestrator.rs`:

- `load_missing_file_is_none`
- `load_trims_text`
- `load_empty_file_is_an_error_naming_the_path`
- `load_file_over_the_limit_is_an_error_with_both_numbers` (a file of `MAX_RULES_CHARS + 1`
  characters; the message contains the path, `MAX_RULES_CHARS + 1` and `MAX_RULES_CHARS`)
- `load_file_at_the_limit_is_whole` (exactly `MAX_RULES_CHARS` characters comes back uncut)
- `user_file_replaces_the_default_process` (the assembled text contains the user's text and its
  path, and does not contain the default text)
- `no_user_file_uses_the_default_and_says_so` (contains `## Process (from default (built in))`)
- `no_default_and_no_user_file_has_no_process_section`
- `repository_file_is_added_after_the_process` (both present; the repository heading comes later)
- `sections_come_in_order` (reference, answers sentence, process, repository)
- `answers_person_and_nobody_differ` (each contains its own constant and not the other)
- `start_lines_name_sources_or_none`

In `test_orchestrator_instructions.rs`, using a temporary directory as the repository root and
`XDG_CONFIG_HOME` pointed at another temporary directory (run the binary as a child process, or call
`build` with the environment set under a lock; do not change the test process's environment without
one):

- `shell_mode_gives_the_reference_and_the_default_process` (contains the first line of
  `reference.md` and of `process.md`)
- `tool_mode_gives_the_tool_reference_and_no_process` (contains the first line of
  `reference-dispatch-tool.md`; does not contain `## Process`; start line is `process: none`)
- `instructions_do_not_contain_the_skill_or_the_old_preamble` (does not contain
  `# blyte as your implementer`, `Start on the work at once`, or
  `Get oriented with a few cheap calls`)
- `an_empty_user_file_stops_the_start` (`build` is `Err` and names the file)
- `repository_rules_for_implementers_are_not_given` (a root with `AGENTS.md` containing a marker
  string: the orchestrator's system prompt, built through `Runner::system_prompt` or the nearest
  public path to it, does not contain the marker)

**Constraints.** Existing tests pass unmodified except: tests that assert text of the removed
preamble, skill injection or dispatch reminder, or call a removed function, are changed to assert
the new behaviour described here, or removed when they tested only the dispatch reminder. List each
such test in your final message. `crates/blyte-cli/assets/` is not changed. The implementer's prompt
is not changed.

**Depends on:** none.

**Siblings.** O3 changes the `bash` tool's description in `crates/blyte-agent/src/tools.rs` and adds
a shell test; it is not in this worktree and you must not touch `tools.rs` or `shell.rs`. O2 adds
the `blyte instructions` command later; do not add it here.

**Commit message.**
`feat(orchestrate): facts built in, the process from a file the user can replace`

with the body `~/.config/blyte/orchestrator.md now replaces the default process instead of being added to it; a file that is empty or over the size limit stops the start.`

### Task O2: `blyte instructions` prints what the orchestrator is given

**Goal.** A user can read the instructions in force, and get the default process to edit. A test
keeps the reference and the default process true to the command line.

**Files.**
- `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`
- `crates/blyte-cli/src/instructions_cmd.rs` (new), `crates/blyte-cli/src/lib.rs` (declare it)
- `crates/blyte-cli/tests/test_instructions_cmd.rs` (new)
- `crates/blyte-cli/tests/test_orchestrator_reference.rs` (new)

**What to build.** A subcommand `instructions` with `--default-process`, `--one-shot` and
`--dispatch-tool`:

- `blyte instructions --default-process` prints `orchestrator_text::DEFAULT_PROCESS` exactly and
  exits 0. It reads no file. It cannot be combined with the other two flags (clap rejects it).
- `blyte instructions` prints, for the repository containing the current directory:
  1. the two start lines from `orchestrator_text::build`, then a blank line;
  2. the built instructions, exactly as the model is given them;
  3. `## Always sent` followed by `blyte_agent::run::ORCHESTRATOR_FIXED`;
  4. `## Tool descriptions`, then for each tool in `blyte_agent::tools::specs()` a line
     `### {name}` and its description;
  5. the line `The request and a description of the environment are added when a run starts.`
- `--one-shot` builds with `Answers::Nobody`; without it, `Answers::Person`. `--dispatch-tool`
  builds as when `--dispatch-cmd` is set.
- An `Err` from `build` prints `error: {message}` to stderr and exits 1. Outside a git repository
  it prints `error: not inside a git repository` and exits 1.

`pub fn render(dispatch_tool: bool, answers: Answers, root: &Path) -> Result<String, String>`
in `instructions_cmd.rs` holds the logic; `main.rs` only prints.

**Tests.** `cargo test -p blyte --test test_instructions_cmd --test test_orchestrator_reference`.

`test_instructions_cmd.rs` (child process with `XDG_CONFIG_HOME` set to a temporary directory and a
temporary git repository as the current directory):
- `default_process_flag_prints_the_file_exactly`
- `prints_start_lines_then_the_reference_and_the_default_process`
- `a_user_file_is_named_and_replaces_the_default`
- `a_repository_file_is_named_and_included` (`.blyte/orchestrator.md` in the repository)
- `one_shot_says_nobody_will_answer`
- `dispatch_tool_prints_the_tool_reference_and_process_none`
- `lists_every_tool_description` (each name from `tools::specs()` appears as `### {name}`)
- `an_oversized_user_file_exits_1_and_names_it`
- `outside_a_repository_exits_1`

`test_orchestrator_reference.rs` checks `reference.md` and `process.md` against the real command
line (`<blyte_cli::cli::Cli as clap::CommandFactory>::command()`):
- `every_blyte_subcommand_named_exists`: every backtick span or fenced code line that starts with
  `blyte ` names, as its next word, a subcommand that exists.
- `every_flag_named_exists_on_its_subcommand`: every `--flag` in such a span or line exists on that
  subcommand or as a global flag. A `--flag` in a backtick span with no `blyte` in it is checked
  against every subcommand and must exist on at least one.
- `the_check_catches_an_unknown_flag`: the same function run on the text
  `` `blyte review T1 --no-such-flag` `` reports `--no-such-flag`.

**Constraints.** Existing tests pass unmodified. The three files under
`crates/blyte-cli/assets/orchestrator/` are not changed: if the reference test finds a command or
flag that does not exist, stop and report it in your final message without editing the text or
weakening the test.

**Depends on:** O1 (dispatch with `--base wip/O1`).

**Siblings.** O3 (the `bash` tool description) is not in this worktree.

**Commit message.** `feat(cli): blyte instructions prints what the orchestrator is given`

### Task O3: the `bash` tool says what background jobs do

**Goal.** The `bash` tool's description says "there is no background mode and nothing to poll",
while the orchestrator's reference says its shell keeps background jobs alive after a command
returns. Find out by test which is true for each way commands are run, and make the tool description
state it.

**Files.** `crates/blyte-agent/src/tools.rs`, `crates/blyte-agent/src/shell.rs` (tests, and the
smallest change needed to tell the description which executor is in use),
`crates/blyte-agent/tests/test_shell_background.rs` (new).

**What to build.**
1. Find which executor a `blyte --orchestrate` run uses for the model's `bash` calls (the persistent
   `Session` in `shell.rs`, or the process-per-command executor) and which the implementer uses.
2. Write `test_shell_background.rs`, one test per executor: run
   `sleep 30 > /dev/null 2>&1 & echo $!` through it, assert the call returns in under 5 seconds with
   a process id, then report whether that process is still alive one second later (`kill -0`), and
   kill it at the end of the test. Name the tests for what they find, for example
   `session_keeps_a_redirected_background_job_alive` and
   `per_command_executor_ends_background_jobs_with_the_command`.
3. Make the `bash` tool description say, for the executor in use, what the test showed: either that
   a background job started with its output redirected keeps running after the command returns and
   that nothing reports when it exits, or that background jobs end with the command. The rest of the
   description is unchanged.
4. If the session does **not** keep a redirected background job alive, change nothing: stop and say
   so in your final message, with the test output. The reference text would then be wrong and is
   not yours to edit.

**Tests.** `cargo test -p blyte-agent --test test_shell_background`, and a unit test in `tools.rs`
for each description variant asserting the sentence it contains and that it does not contain the
other.

**Constraints.** Existing tests pass unmodified, except a test that asserts the old sentence of the
`bash` description, which is updated to the new sentence. No other tool description changes.

**Depends on:** none.

**Siblings.** O1 rewrites `orchestrator.rs` and edits `run.rs`, `task_runner.rs` and `repl.rs`; it
is not in this worktree and you must not touch those files.

**Commit message.** `fix(agent): the bash tool says what background jobs do in its shell`

## The trial (run by the reviewer after O1–O3 are combined)

As the spec's "Measuring the corrections". Build the branch's binary, choose the task and the commit
before the first run, and run each orchestrator in its own clone so the runs cannot see each other.
Arm B's process file is `process.md` with this appended, the four corrections that the default
process does not already contradict:

```
### Corrections under test

- Do not read blyte's source, old runs, or `.wt/` files other than the ones the reference names, and
  do not run `blyte --version` or `which blyte` to find out how dispatch works.
- The reference documents every blyte command you need, so do not run `--help`, and do not check the
  environment or `~/.config/blyte` for API keys: `blyte dispatch` reports a missing key itself.
- Keep tool output small, since everything you run stays in the conversation: read
  `blyte review <id>` before any `--diff`, and trim long output with `| tail -n 40` or `grep`.
- Start on the work at once.
```

Arm B sets `XDG_CONFIG_HOME`; check first whether that also moves where blyte looks for its `env`
file, and if it does, give both arms the API key through the environment. Results, with their limits, go to `docs/evidence.md`.
