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

### Task OC3: the conversation continues across prompts in interactive sessions

**Goal.** In interactive `blyte`, every prompt starts a fresh model conversation: `Runner::run_until`
(`crates/blyte-agent/src/run.rs:586`) builds `history = vec![Message::User(format!("Begin. {goal}"))]` each
time, and `State` keeps no messages. So when the model asks the user a question and the user answers, the
answer arrives without the question. Keep the conversation in the session's `State` and continue it on
the next prompt. One-shot runs and `blyte dispatch` must behave exactly as today.

**Files.** `crates/blyte-agent/src/state.rs`, `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/repl.rs`,
a new test file `crates/blyte-agent/tests/test_conversation.rs`, and `crates/blyte-cli/tests/test_repl_commands.rs`.

**What to build.**
- `state.rs`, `Runtime` gains two fields (drop `Eq` from `Runtime`'s derives if `Message` is not `Eq`; keep
  `PartialEq`):
  ```rust
  /// The conversation so far, kept across prompts in an interactive session. `None`: each run starts
  /// fresh (one-shot runs and dispatches). `Some`: the run continues it and stores it back.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub conversation: Option<Vec<blyte_provider::Message>>,
  /// The goal of the conversation's first prompt; the system prompt is built from it for the whole
  /// conversation, so it stays identical and its cache holds.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub conversation_goal: Option<String>,
  ```
  `Runtime::default()` has both `None`. (Check `blyte-agent` already depends on `blyte-provider`; `run.rs`
  imports `blyte_provider::Message`.)
- `run.rs`, `Runner::run_until`:
  - If `state.runtime.conversation` is `None`: exactly today's behaviour.
  - If it is `Some(prev)` and `prev` is empty: start as today (`"Begin. {goal}"`), and set
    `conversation_goal = Some(goal)`.
  - If it is `Some(prev)` and `prev` is not empty: `history = prev.clone()` followed by
    `Message::User(goal)` (the new prompt, verbatim, without "Begin.").
  - The system prompt is built with `conversation_goal` as the goal when it is `Some`, otherwise with
    `state.orchestrator.goal` (today). Change `system_prompt` to take the goal as a parameter if that is
    simplest.
  - On **every** return from `run_until` (verified, gate failed, budget exhausted, errors, interrupted;
    there are about seven `return`s), when `conversation` is `Some`, store the final `history` back into it.
    Do this structurally: move the loop body into an inner function that takes `&mut Vec<Message>` and
    returns `(Outcome, Report)`, and have `run_until` build the history, call it, and store the result.
  - The turn budget, turn numbering (`Event::Turn { n }` from 1) and the late-budget reminder are per
    prompt, as today.
  - The context strategy (`self.context.build(...)`) gets the whole continued history, so `folded` folds
    across prompts with its first message (the first prompt) kept as the opening.
- `repl.rs`:
  - When the session is created (`ReplSession::new`, around line 49-52), set
    `state.runtime.conversation = Some(Vec::new())`.
  - `/clear` (line ~424) resets `runtime` to default and then sets `conversation = Some(Vec::new())` again,
    so the next prompt starts a new conversation. Its message becomes
    `"Conversation cleared; the next prompt starts a new one."`.
  - `/state` (line ~371) must not print the messages: print the runtime with `conversation` replaced by a
    line `conversation: N messages` (and `conversation_goal` as is).

**Tests.**
- `crates/blyte-agent/tests/test_conversation.rs`, using a scripted provider that records each `Request`
  (copy the pattern from `crates/blyte-agent/tests/test_parallel_calls.rs` or `test_folded.rs`), the
  `Conversational` context and no gate (ungated runs stop when the model replies without tool calls):
  - `second_prompt_continues_the_conversation`: `State` with `conversation = Some(vec![])`; run with goal
    "first", the model replies text "Which file?"; set `state.orchestrator.goal = "src/a.rs"` and run again;
    the second run's first request contains, in order, `User("Begin. first")`, an assistant message with
    "Which file?", and `User("src/a.rs")`.
  - `system_prompt_is_stable_across_prompts`: in the same setup, the system prompt of the second run's first
    request equals the first run's, and contains "first", not "src/a.rs".
  - `without_conversation_each_run_is_fresh`: `conversation = None`; two runs; the second run's first request
    has exactly one message, `User("Begin. src/a.rs")`; `state.runtime.conversation` is still `None` after.
  - `conversation_is_stored_after_budget_exhausted`: `max_turns: 1`, the model calls a tool every turn; after
    the run `conversation` is `Some` and holds the first prompt, the tool call and its result.
  - `serialized_runtime_without_conversation_is_unchanged`: `serde_json::to_value(Runtime::default())` has
    no `conversation` key (dispatch reports stay the same).
- `crates/blyte-cli/tests/test_repl_commands.rs`:
  - `test_clear_starts_new_conversation`: a session whose `state.runtime.conversation` holds one message;
    `/clear` leaves it `Some(vec![])` and prints "Conversation cleared".
  - `test_state_counts_conversation_messages`: with two messages, `/state` output contains
    `conversation: 2 messages` and not the messages' text.

Run: `cargo test -p blyte-agent --test test_conversation` and `cargo test -p blyte-cli --test test_repl_commands`.

**Constraints.** Existing tests pass unmodified (add the new fields wherever a `Runtime { .. }` literal needs
them, nothing else). No new dependencies. No change to `task_runner.rs`, `dispatch.rs` or `cli.rs`.

**Depends on:** OC1 (merged).

**Siblings.** OC4 adds `crates/blyte-agent/src/orchestrator.rs` and a line in `crates/blyte-agent/src/lib.rs`
in a separate worktree; do not create them. OC2/OC2b add `blyte wait`; not here.

**Commit message:** `feat(repl): keep the conversation across prompts so a reply to the model's question has its context`

### Task OC4: the orchestrator's instructions

**Goal.** `blyte --orchestrate` (OC5) needs its system-prompt text: a short orchestrator preamble, the
blyte-implementer skill, and the user's own rules from `~/.config/blyte/orchestrator.md`. Build that text in
one new module so OC5 only wires it in.

**Files.** Create `crates/blyte-agent/src/orchestrator.rs`; add `pub mod orchestrator;` to
`crates/blyte-agent/src/lib.rs` (keep the list's order). Nothing else.

**What to build** (in `orchestrator.rs`):
```rust
//! The instructions for `blyte --orchestrate`: blyte as the planner and reviewer that dispatches
//! implementers, as Claude Code does with the blyte-implementer skill.

/// What the orchestrator is. Stands in for the implementer's opening line.
pub const PREAMBLE: &str = "\
You are the orchestrator in a repository. You plan the work, dispatch implementers with \
`blyte dispatch`, review what they did with `blyte review`, record a verdict, and merge. You do not \
write the implementation yourself, except for a one-line fix; every other change goes to a dispatch.

You work with a person. When you need a decision that is theirs to make, or you have a plan for them \
to approve, end your reply with the question and stop: their answer arrives as the next message. Do not \
ask about choices with a conventional default; pick it, say so, and carry on.

Keep tool output small, since everything you run stays in the conversation: send dispatch output to \
`/dev/null` (each run's summary is in `.wt/<id>.json`), wait with `blyte wait`, read `blyte review <id>` \
before any `--diff` and open the diff only for what it flags, and trim long output with `| tail -n 40` or \
`grep`. When you need a detail from earlier that is no longer in view, re-read it from the plan file or \
`.wt/<id>.task.md` rather than guessing.";

/// The user's rules file: `$XDG_CONFIG_HOME/blyte/orchestrator.md` when `XDG_CONFIG_HOME` is set and not
/// empty, else `$HOME/.config/blyte/orchestrator.md`; `None` when neither variable is set.
pub fn user_rules_path() -> Option<std::path::PathBuf>

/// The file's text, trimmed; `None` if it is missing, unreadable or empty. Longer than
/// `crate::rules::MAX_RULES_CHARS` characters: cut there, followed by
/// `"\n\n[cut at {MAX_RULES_CHARS} characters; the rest is in {path}]"` (path displayed in full).
pub fn load_user_rules(path: &std::path::Path) -> Option<String>

/// The orchestrator sections of the system prompt, in this order:
/// "## How you work\n{PREAMBLE}\n\n## The blyte-implementer skill\n{skill}\n\n", then, if `user_rules`
/// is `Some((path, text))`, "## Your rules (from {path})\n{text}\n\n". `skill` is inserted trimmed.
pub fn prompt_sections(skill: &str, user_rules: Option<(&std::path::Path, &str)>) -> String
```
Copy `PREAMBLE` verbatim. `MAX_RULES_CHARS` is the existing public constant in `crates/blyte-agent/src/rules.rs`.

**Tests** (unit tests in `orchestrator.rs`, `tempfile` for files):
- `user_rules_path_prefers_xdg`, `user_rules_path_falls_back_to_home`, `user_rules_path_ignores_empty_xdg`,
  `user_rules_path_none_without_home`: test a private helper
  `fn user_rules_path_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf>` that
  `user_rules_path` calls with the real variables, so the tests do not set environment variables.
- `load_missing_is_none`, `load_blank_is_none` (a file of spaces and newlines), `load_trims`
  (`"\n  be brief \n"` → `"be brief"`).
- `load_caps_long_file`: `MAX_RULES_CHARS + 10` copies of `"é"` → the result starts with exactly
  `MAX_RULES_CHARS` `"é"`s followed by `"\n\n[cut at "` and ends with the path and `"]"`.
- `sections_in_order`: `prompt_sections("  SKILL BODY \n", Some((path, "RULES BODY")))` contains
  `"## How you work"`, `"## The blyte-implementer skill\nSKILL BODY\n"` and `"## Your rules (from "`, in that
  order, and contains `PREAMBLE`.
- `sections_without_rules`: `prompt_sections("S", None)` has no `"## Your rules"`.
- `preamble_names_the_commands`: `PREAMBLE` contains `blyte dispatch`, `blyte review`, `blyte wait` and
  `/dev/null`.

Run: `cargo test -p blyte-agent orchestrator`.

**Constraints.** No new dependencies. No other file changes; nothing calls the module yet (OC5 does). If clippy
flags the module as unused, it will not: the items are `pub`.

**Depends on:** none.

**Siblings.** OC3 changes `state.rs`, `run.rs` and `repl.rs` in a separate worktree; do not touch them.

**Commit message:** `feat(agent): orchestrator instructions: preamble, skill and the user's orchestrator.md`

### Task OC5: `blyte --orchestrate`

**Goal.** `blyte --orchestrate -m <model>` starts interactive blyte as the orchestrator: it plans, dispatches
implementers with `blyte dispatch`, waits with `blyte wait`, reviews and merges, as Claude Code does with the
blyte-implementer skill. This task wires together what OC2 (`blyte wait`), OC3 (the conversation continues
across prompts) and OC4 (`blyte_agent::orchestrator`) built. Spec: `docs/superpowers/specs/2026-10-07-orchestrate-mode.md`.

**Files.** `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`, `crates/blyte-cli/src/repl.rs`,
`crates/blyte-agent/src/state.rs`, `crates/blyte-agent/src/run.rs`,
`crates/blyte-cli/assets/blyte-implementer/SKILL.md`, `docs/orchestration.md`, a new test file
`crates/blyte-cli/tests/test_orchestrate.rs`, a new test file `crates/blyte-agent/tests/test_orchestrator_prompt.rs`,
and literal updates in `crates/blyte-cli/tests/test_repl_commands.rs` / `test_repl_model_cmd.rs` only where a new
`ReplConfig` field must be added.

**What to build.**

1. **Flag** (`cli.rs`, on `Cli`, next to `--gate`):
   ```rust
   /// Run interactive blyte as the orchestrator: plan, dispatch implementers with `blyte dispatch`, review and merge
   #[arg(long, conflicts_with = "gate")]
   pub orchestrate: bool,
   ```
   And in `cli.rs`:
   ```rust
   /// Turn and context defaults for `--orchestrate`. A flag the user gave explicitly wins.
   pub fn apply_orchestrate_defaults(cli: &mut Cli, matches: &clap::ArgMatches)
   ```
   When `cli.orchestrate`: if `max_turns` came from its default (`matches.value_source("max_turns") ==
   Some(clap::parser::ValueSource::DefaultValue)`), set it to 1000; if `fold_budget` is `None`, set
   `Some(100_000)`. `context` keeps its default `folded`. Does nothing when `orchestrate` is false. Do not
   change the existing fields' types or defaults (existing tests assert them).
2. **main.rs:** parse with `let matches = Cli::command().get_matches(); let mut cli =
   Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());` (imports `clap::{CommandFactory,
   FromArgMatches}`), then `apply_orchestrate_defaults(&mut cli, &matches)`. Behaviour without
   `--orchestrate` is unchanged. If `cli.orchestrate` and a prompt is given (`cli.prompt.is_some()`): print
   `error: --orchestrate is interactive; start it without a prompt` to stderr and exit 2. Pass
   `orchestrate: cli.orchestrate` and `fold_budget: cli.fold_budget` into `ReplConfig`.
3. **`state.rs`:** `Orchestrator` gains
   ```rust
   /// Set for `blyte --orchestrate`: the orchestrator's instructions, which replace the implementer's
   /// framing in the system prompt.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub instructions: Option<String>,
   ```
   (`None` in `State::new`.)
4. **`run.rs`, `system_prompt`:** when `state.orchestrator.instructions` is `Some(text)`, return instead:
   ```
   {text}## Your user's first request\n{goal}\n\n## Environment\n{where_}\nAvailable: {bins}\n\n
   Change nothing outside the repository yourself (blyte's own files under `.wt/` and its records are fine).\n\n
   {rules}## Working efficiently\n<the same batching paragraph as today>
   ```
   where `goal` is the goal the system prompt already uses (OC3's `conversation_goal` when set). It has no
   "You are a software engineer" line and no "How completion is decided" section. Without `instructions`
   the prompt is byte-for-byte what it is today.
5. **`repl.rs`:**
   - `ReplConfig` gains `pub orchestrate: bool` and `pub fold_budget: Option<usize>` (`false`/`None` in
     `Default`); `ReplSession` gains `pub orchestrate: bool` and `pub fold_budget: Option<usize>`.
   - `ReplSession::new`: when `config.orchestrate`, set `state.orchestrator.instructions =
     Some(blyte_agent::orchestrator::prompt_sections(crate::skill::SKILL_MD, rules))`, where `rules` comes
     from `blyte_agent::orchestrator::user_rules_path()` and `load_user_rules` (as `Some((path, text))` when
     the file has text). The session's goal string stays as it is.
   - `SubmitPrompt`: `sandbox` is `"off"` when `session.orchestrate`, else `"on"` (today); `network` stays
     `"on"`; `fold_budget: session.fold_budget`.
   - After each reply, when `session.orchestrate`, print only the usage line instead of "Task verified in…"/
     "Task finished with outcome…": `format_usage_line(report.outcome_turns, &report.usage)`, a new `pub fn`
     returning `"{turns} turns · {input} in ({cached} cached) · {output} out"` (token counts from `Usage`,
     e.g. `"3 turns · 12000 in (9000 cached) · 450 out"`). If the outcome is not `Verified`, prefix
     `"Stopped ({outcome:?}): "`. Without `--orchestrate` these lines are unchanged.
   - `/gate <cmd>` when `session.orchestrate`: print `No gate in orchestrate mode.` and change nothing.
   - The banner or `/help` mentions orchestrate mode when it is on (one line).
6. **SKILL.md**, section "Starting parallel tasks" (around lines 104-117): after the `wait` code block, add:
   ````markdown
   **Under `blyte --orchestrate`** (your commands run in a shell that keeps background jobs alive, but you
   are not told when they exit, and each command times out after 300 s): start each dispatch in the
   background with both outputs redirected, or the command waits for it, then wait with `blyte wait`:

   ```bash
   blyte dispatch T1 --plan plan.md --plan-task "Task T1" > /dev/null 2>&1 &
   blyte dispatch T2 --plan plan.md --plan-task "Task T2" > /dev/null 2>&1 &
   blyte wait --any T1 T2
   ```

   `blyte wait` prints `<id> <outcome> <turns> <cost>` for each run that has finished. Review those, drop them
   from the list and call it again for the rest. Exit 2 means none finished within its timeout (280 s): call
   it again.
   ````
   (Use a four-backtick fence or indentation in the real file so the inner fence nests correctly.)
7. **`docs/orchestration.md`:** a section `## blyte as the orchestrator` (about 15 lines): starting it
   (`blyte --orchestrate -m vertex/gemini-4-argon`), what it does, that its own shell is not sandboxed while
   every implementer it dispatches is, the 1000-turn and 100k-token folded defaults and their overrides, the
   conversation kept across prompts and `/clear`, `~/.config/blyte/orchestrator.md`, and `blyte wait`.

**Tests.**
- `crates/blyte-cli/tests/test_orchestrate.rs`:
  - `orchestrate_sets_defaults`: `Cli::command().try_get_matches_from(["blyte", "--orchestrate"])` → after
    `apply_orchestrate_defaults`, `max_turns == 1000`, `fold_budget == Some(100_000)`, `context == "folded"`.
  - `explicit_flags_win`: `["blyte", "--orchestrate", "--max-turns", "30", "--fold-budget", "5000"]` → 30 and
    `Some(5000)`.
  - `without_orchestrate_nothing_changes`: `["blyte"]` → `max_turns == 25`, `fold_budget == None`.
  - `orchestrate_conflicts_with_gate`: `["blyte", "--orchestrate", "--gate", "true"]` is a parse error.
  - `orchestrate_with_prompt_is_rejected`: run the built binary (`env!("CARGO_BIN_EXE_blyte")`) with
    `--orchestrate "do it"` → exit code 2, stderr contains `--orchestrate is interactive`.
  - `session_gets_instructions`: `ReplSession::new(&ReplConfig { orchestrate: true, ..Default::default() })`
    → `state.orchestrator.instructions` contains `"## The blyte-implementer skill"` and `"blyte wait"`; with
    `orchestrate: false` it is `None`.
  - `gate_refused_in_orchestrate`: `/gate true` on an orchestrate session prints `No gate in orchestrate mode.`
    and leaves `session.gate` `None`.
  - `usage_line_format`: `format_usage_line(3, &Usage { input: 12000, cached: 9000, output: 450, thinking: 0 })`
    is exactly `"3 turns · 12000 in (9000 cached) · 450 out"`.
  - `skill_tells_orchestrator_to_redirect_and_wait`: `SKILL_MD` contains `"Under \`blyte --orchestrate\`"`,
    `"> /dev/null 2>&1 &"` and `"blyte wait --any"`.
- `crates/blyte-agent/tests/test_orchestrator_prompt.rs` (a recording provider, as in
  `tests/test_parallel_calls.rs`):
  - `instructions_replace_the_implementer_framing`: a `State` with `instructions = Some("## How you work\nX\n\n")`
    → the system prompt starts with `"## How you work\nX"`, contains `"## Your user's first request"` and the
    goal, and contains neither `"You are a software engineer"` nor `"How completion is decided"`.
  - `without_instructions_the_prompt_is_unchanged`: the system prompt contains `"You are a software engineer"`
    and `"How completion is decided"`.
  - `orchestrator_state_serializes_without_instructions`: `serde_json::to_value(State::new("g", ""))` has no
    `instructions` key.

Run: `cargo test -p blyte-cli --test test_orchestrate` and `cargo test -p blyte-agent --test test_orchestrator_prompt`.

**Constraints.** Existing tests pass unmodified, except that a `ReplConfig { .. }` or `Orchestrator { .. }`
literal may gain the new fields. No new dependencies. No change to `dispatch.rs`, `wait.rs`, `task_runner.rs`
or `orchestrator.rs`.

**Depends on:** OC2, OC2b, OC3, OC4 (all on `main` before this is dispatched).

**Commit message:** `feat(repl): blyte --orchestrate runs interactive blyte as the orchestrator that dispatches implementers`

## Follow-ups from the real check (2026-10-07)

### Task OC6: show a tool's whole command on its event line

**Goal:** the `● bash <input> -> ...` line cuts the command at 70 characters (`run.rs`, the `input` built
just before `self.emit(Event::Tool {`), so the user cannot see what the orchestrator ran (a failed
`blyte dispatch A1 --plan docs/plan.md --plan-task "Task A1: mathx.clam` was unreadable). Show it in full.

**Files:** `crates/blyte-agent/src/run.rs`; new test `crates/blyte-agent/tests/test_tool_event_input.rs`.

**What to build:** in the non-`read` branch, the input is the value of `command`, else `pattern`, else
`path` (as a string), else `call.arguments.to_string()`, with these changes only:
- every `\n` is replaced by ` ↵ ` (space, U+21B5, space), and trailing whitespace is trimmed first;
- the result is cut at 2000 characters (counted with `chars()`), and when cut, `…` is appended.
Leave the `read` branch (100 characters) and the summary (90 characters) as they are.

**Tests** (model the file on `crates/blyte-agent/tests/test_read_event_range.rs`: a scripted provider, a
`Runner` with `Conversational`, collecting `Event::Tool` inputs):
1. a bash call whose `command` is a 150-character single line: the event input equals it exactly;
2. a bash call with `command` `"echo one\necho two\n"`: input is `echo one ↵ echo two`;
3. a bash call with a 2500-character `command`: input has 2001 chars and ends with `…`, and its first
   2000 chars equal the command's first 2000.
Run: `cargo test -p blyte-agent --test test_tool_event_input`.

**Constraints:** existing tests pass unmodified.

**Depends on:** none. **Siblings:** OC7 (orchestrator.rs) and OC8 (wait.rs) run in parallel in other
worktrees; do not touch those files.

**Commit message:** `fix(agent): show a tool's whole command on its event line instead of the first 70 characters`

### Task OC7: the orchestrator starts work straight away

**Goal:** in the real check the orchestrator spent 13 turns running `blyte dispatch --help`,
`blyte review --help`, grepping the environment for API keys and listing `~/.config/blyte` before its
first dispatch. The skill already documents the commands, and dispatch reports a missing key itself.

**Files:** `crates/blyte-agent/src/orchestrator.rs`; `crates/blyte-agent/tests/test_orchestrator_prompt.rs`.

**What to build:** append this paragraph to `PREAMBLE`, after the existing last paragraph and separated
from it by one blank line (`\n\n`), wrapped with `\` line continuations like the rest:

"Start on the work at once. The skill below documents every blyte command you need, so do not run
`--help`, and do not check the environment or `~/.config/blyte` for API keys: `blyte dispatch` reports a
missing key itself. When the request names a plan, read it and dispatch its independent tasks in your
first turns."

**Tests:** add to `test_orchestrator_prompt.rs`: `PREAMBLE` contains `do not run \`--help\`` and
`reports a missing key itself`, and `prompt_sections(...)` output contains that paragraph. Existing
tests in the file pass unmodified.
Run: `cargo test -p blyte-agent --test test_orchestrator_prompt`.

**Depends on:** none. **Siblings:** OC6 (run.rs), OC8 (wait.rs) in other worktrees; do not touch them.

**Commit message:** `fix(agent): tell the orchestrator to start on the plan instead of reading --help`

### Task OC8: `blyte wait` gives a just-started dispatch time to appear

**Goal:** the orchestrator ran `blyte dispatch A1 ... > /dev/null 2>&1 & blyte dispatch A2 ... & ...`
followed at once by a blyte command on those ids, and it exited 1 though both runs started. `blyte
dispatch` writes `.wt/<id>.start.json` only after it has created the worktree and spawned the agent
(`dispatch.rs`, around line 494), so `blyte wait A1 A2` straight after `&` finds neither file and
prints `no dispatch A1 here`, exit 1 (`wait.rs`, the "unknown" check before the loop).

**Files:** `crates/blyte-cli/src/wait.rs` only.

**What to build:** in `wait_in`, replace the one-off unknown check: an id that is not yet known
(`is_known` false) is treated as pending, polled every `poll` like the others, for up to `grace` from
the start of the call. If any id is still unknown once `grace` has passed, write `no dispatch <id> here`
to `err` (one line per such id, in argument order) and return 1, printing nothing to `out` for the
unknown ids. Ids that become known are handled exactly as today (including their own dead-run grace,
counted from when they were first seen as looking dead, as today). The overall `timeout` still applies:
if it passes first, behave as today's timeout (exit 2, `still running: ...` listing the ids not
finished, unknown ones included). `run` keeps passing its 15 s grace.

**Tests** (in `wait.rs`'s test module, using the existing helpers there):
1. update the existing unknown-id test to pass a small `grace` (e.g. 50 ms) and assert exit 1 and
   `no dispatch T9 here`, and that it returned in under 1 s;
2. a summary `.wt/<id>.json` (no start file) written by a thread 100 ms after `wait_in` starts (grace 2 s, poll
   10 ms): exits 0 and prints that run's summary line;
3. two ids, one with a summary, one never appearing (grace 50 ms): exit 1, `no dispatch` names only the
   missing one.
Every other existing test passes; adjust only arguments, not assertions, where a test passed
`Duration::ZERO` for grace and relies on an unknown id.
Run: `cargo test -p blyte --lib wait`.

**Depends on:** none. **Siblings:** OC6 (run.rs), OC7 (orchestrator.rs) in other worktrees; do not
touch them.

**Commit message:** `fix(wait): give a dispatch started just before blyte wait time to write its start file`

### Task OC9: say why a plan heading was not found

**Goal:** `blyte dispatch --plan p.md --plan-task "Task X"` printed only `error: no heading "Task X" in
p.md` when the heading existed but sat after a code block that an inner fence had closed early, so a
later fence line opened a block that never closed and every heading after it was skipped (2026-10-07,
this plan's OC5 section). Say so, with line numbers, so the user can fix the plan.

**Files:** `crates/blyte-cli/src/plan_section.rs`, `crates/blyte-cli/src/dispatch.rs` (the
`None =>` arm after `crate::plan_section::extract`, around line 214).

**What to build:**
- In `plan_section.rs`, `pub fn explain_missing(plan: &str, task: &str) -> Option<String>`, used only
  after `extract` returned `None`. It scans the plan with the same fence rules as `extract` (move the
  heading-text parsing into a shared private helper rather than copying it). Line numbers are 1-based.
  - If a line inside a fenced block would be a heading matching `task` (same `heading_matches` rule),
    return `the heading at line {H} is inside a code block that opens at line {F}; an inner fence may
    have closed an outer one early (nest with a longer fence such as ```` around ```)`, for the first
    such heading. `F` is the line of the fence that opened the block containing line `H`.
  - Else, if the plan ends inside an open fenced block, return `the code block that opens at line {F}
    is never closed, so headings after it are not read`.
  - Else `None`.
- In `dispatch.rs`, after the existing error line, when `explain_missing` returns `Some(hint)`, print a
  second line `  {hint}` to stderr. Exit code stays 2.

**Tests** (in `plan_section.rs`'s test module):
1. a plan with "```markdown" at line 3, "```bash" at 5, "```" at 7, "```" at 9 (the OC5 shape) and
   `### Task X: y` at line 11, with no fence after: `extract(plan, "Task X")` is `None` and
   `explain_missing` returns the "inside a code block that opens at line 9" message naming line 11;
2. a plan whose only heading `### Task X` is inside a properly closed fenced block (```...```) and no
   other: the message names that heading's line and the block's opening line;
3. a plan with an unclosed fence at line 2 and no matching heading anywhere: the "never closed" message
   naming line 2;
4. a plan with no fences and no matching heading: `None`;
5. a heading inside a `~~~` block is reported the same way as test 2.
Existing tests pass unmodified.
Run: `cargo test -p blyte --lib plan_section`.

**Depends on:** none.

**Commit message:** `fix(dispatch): say when a plan heading is hidden inside an unclosed code block`

### Task OC10: a Gemini reply with text and tool calls goes back as one model turn

**Goal:** when Gemini answers with text and tool calls in one response, the loop stores
`Message::Assistant(text)` followed by `Message::ToolCall`s (`run.rs`, around lines 784-795), and
`Gemini::body` sends them back as two consecutive `"role": "model"` contents, because only a preceding
`ToolCall` makes a call join the previous content (`crates/blyte-provider/src/gemini.rs`, the
`Message::ToolCall` arm of `body`). Gemini returned them as one content; send them back the same way so
user and model turns alternate.

**Files:** `crates/blyte-provider/src/gemini.rs`; `crates/blyte-provider/tests/test_gemini_wire.rs`.

**What to build:** in the `Message::ToolCall` arm, join the previous content with `push_part` when `prev`
is `Some(Message::ToolCall { .. })` **or** `Some(Message::Assistant(_))`; otherwise push a new model
content as today. Update the comment above it to say both cases. Change nothing else (signatures are
still put on the functionCall part exactly as now).

**Tests** (add to `test_gemini_wire.rs`, using its `gemini()` helper; build `ToolCall`s as the existing
tests there do):
1. messages `[User("go"), Assistant("I'll look"), ToolCall(read_file, signature Some("sig")),
   ToolResult]`: `contents` has 3 entries with roles `user`, `model`, `user`; the model content's
   `parts` is `[{"text": "I'll look"}, {"functionCall": ..., "thoughtSignature": "sig"}]` in that order;
2. messages `[User, Assistant("a"), ToolCall(c1), ToolCall(c2), ToolResult, ToolResult]`: one model
   content with 3 parts (text, c1, c2) and one user content with 2 functionResponse parts;
3. messages `[User("q"), Assistant("answer"), User("next")]`: 3 contents, roles user, model, user
   (an assistant text with no call is unchanged);
4. no two consecutive entries of `contents` share a role in any of the above (assert it in each test).
Existing tests pass unmodified.
Run: `cargo test -p blyte-provider --test test_gemini_wire`.

**Depends on:** none. **Siblings:** OC9 (plan_section.rs, dispatch.rs) runs separately; do not touch it.

**Commit message:** `fix(gemini): send a reply's text and tool calls back as one model turn`

## Usage log and combined costs (2026-10-07)

Every blyte model session (dispatch, research, one-shot, each interactive prompt, the orchestrator)
appends one line to a machine-wide log, so costs survive worktree cleanup and the orchestrator's own
spend is counted. An interactive session sets `BLYTE_SESSION`, which every dispatch it starts inherits,
so a session's orchestrator and implementer costs can be added up exactly.

### Task U1: the usage log

**Goal:** a durable record of every run's tokens and cost, independent of `.wt/`.

**Files:** new `crates/blyte-cli/src/usage_log.rs`; `crates/blyte-cli/src/lib.rs` (`pub mod usage_log;`);
`crates/blyte-cli/src/task_runner.rs` (the end of the function that calls `runner.run(state)`, around
line 393, where the `--report` record is written).

**What to build** in `usage_log.rs`:
- `pub fn default_path() -> Option<PathBuf>`: `$BLYTE_USAGE_LOG` if set and not empty (the value `off`
  means no log: return `None`); else `$XDG_STATE_HOME/blyte/usage.jsonl` when `XDG_STATE_HOME` is set and
  not empty; else `$HOME/.local/state/blyte/usage.jsonl`; `None` when none of these is available. Put
  the logic in `fn path_from(log: Option<&OsStr>, xdg: Option<&OsStr>, home: Option<&OsStr>)` for tests.
- `#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)] pub struct Entry { ts: u64 (Unix seconds),
  repo: String, session: Option<String>, dispatch: Option<String>, research: bool, model: String,
  outcome: String, turns: u32, input: u64, cached: u64, output: u64, thinking: u64, cost_usd: Option<f64> }`
  (all fields `pub`; `session` and `dispatch` skipped when `None`).
- `pub fn repo_of(root: &Path) -> String`: `root` as a string, except that when it has a path component
  `.wt`, everything before that component (so `/r/.wt/T1` gives `/r`).
- `pub fn append(path: &Path, entry: &Entry) -> std::io::Result<()>`: creates the parent directory, opens
  with create + append, and writes the JSON plus `\n` in a single `write_all` (one line is far below the
  size at which appends could interleave).
- `pub fn read(path: &Path) -> Vec<Entry>`: every line that parses; skips the rest; empty when the file
  is missing.

In `task_runner.rs`, after `runner.run(state)` and whether or not `--report` is set: build an `Entry`
(`ts` now; `repo` = `repo_of(&config.root)`; `session` = env `BLYTE_SESSION`; `dispatch` = env
`BLYTE_DISPATCH_ID`; `research` = `config.research`; `model` = the provider's `model_id`; `outcome` =
`format!("{outcome:?}").to_lowercase()`; `turns` = `report.outcome_turns`; tokens from `report.usage`;
`cost_usd` = `blyte_provider::pricing(&model_id).map(|p| p.cost(&report.usage))`) and `append` it to
`default_path()`. A failed append prints one line `warning: usage log: <error>` to stderr and never
changes the run's result.

**Tests** (in `usage_log.rs`):
1. `path_from`: `BLYTE_USAGE_LOG=/x/u.jsonl` wins; `off` gives `None`; XDG gives `<xdg>/blyte/usage.jsonl`;
   HOME gives `<home>/.local/state/blyte/usage.jsonl`; empty strings are treated as unset; none gives `None`;
2. `repo_of`: `/r/.wt/T1` gives `/r`; `/r` gives `/r`; `/r/.wtx/a` gives `/r/.wtx/a`;
3. `append` twice into a temp dir whose parent does not exist yet, then `read` returns both entries equal
   to what was written, in order; a line of garbage between them is skipped;
4. an entry with `session: None` serialises without a `session` key.
And one integration test in `crates/blyte-cli/tests/` (new file `test_usage_log.rs`), if the existing
tests there run `task_runner` against a scripted provider: with `BLYTE_USAGE_LOG` pointing at a temp
file and `BLYTE_SESSION=s1`, one run appends exactly one entry with `session == Some("s1")`. If there is
no such harness, say so in the report and skip this test.
Run: `cargo test -p blyte usage_log`.

**Constraints:** no new dependencies (`serde`/`serde_json` are already used). Existing tests pass
unmodified; tests that run tasks must not write to the real `~/.local/state`: set `BLYTE_USAGE_LOG=off`
wherever the test suite already isolates `HOME` or the environment, if a test would otherwise write there.

**Depends on:** none. **Siblings:** U2 (dispatch.rs, repl.rs) runs in parallel in another worktree and
sets the two environment variables; do not set them here.

**Commit message:** `feat(usage): log every run's tokens and cost to a machine-wide usage file`

### Task U2: session and dispatch ids in the environment

**Goal:** let the usage log attribute runs: an interactive session gets an id that its shell commands,
and so the dispatches it starts, inherit; a dispatch tells its agent child its id.

**Files:** `crates/blyte-cli/src/repl.rs`, `crates/blyte-cli/src/dispatch.rs`.

**What to build:**
- `repl.rs`: when an interactive session starts, if `BLYTE_SESSION` is unset or empty, set it in the
  process environment (`std::env::set_var`, at startup before any thread is spawned) to
  `format!("{}-{}", unix_seconds, std::process::id())`. Keep the value on the session; `/state` prints a
  line `session: <id>`. Put the id format in `pub fn new_session_id(now_secs: u64, pid: u32) -> String`.
- `dispatch.rs`: where the child `Command` is built (after the `[dispatch.env]` loop, around line 414),
  add `cmd.env("BLYTE_DISPATCH_ID", &args.id)`. `BLYTE_SESSION` passes through by inheritance; do not
  clear it.
- Check that the shell blyte runs commands in (`crates/blyte-agent/src/shell.rs`, the environment scrub)
  does not remove `BLYTE_SESSION`; if it does, say so in the report and do not change shell.rs.

**Tests:** `new_session_id(1700000000, 42) == "1700000000-42"`; in `repl.rs` tests, `/state` output
contains `session: `. For dispatch, a unit test of whatever function builds the child command, if one
exists, asserting `BLYTE_DISPATCH_ID` is set; otherwise say in the report how you checked it.
Run: `cargo test -p blyte repl` and `cargo test -p blyte dispatch`.

**Depends on:** OC9 (also edits dispatch.rs; dispatched with `--base wip/OC9` or after it is merged).
**Siblings:** U1 (usage_log.rs, task_runner.rs) runs separately; do not touch those files.

**Commit message:** `feat(usage): give interactive sessions and dispatches ids the usage log can attribute`

### Task U3: `blyte cost` and the combined line in the REPL

**Goal:** show orchestrator and implementer costs together.

**Files:** new `crates/blyte-cli/src/cost.rs`; `lib.rs`; `cli.rs` and `main.rs` (a `Cost` subcommand);
`repl.rs` (usage line and `/cost`).

**What to build:**
- `cost.rs`: `pub struct Totals { pub orchestrator_usd: f64, pub implementers_usd: f64, pub runs: usize }`
  and `pub fn totals(entries: &[Entry], session: Option<&str>, since: Option<u64>, repo: Option<&str>)
  -> Totals`: keep entries matching every given filter (`session` equal; `ts >= since`; `repo` equal);
  an entry with `dispatch: Some(_)` counts to `implementers_usd` and to `runs`, otherwise to
  `orchestrator_usd`; a `None` cost counts 0.
- `blyte cost [--session ID] [--since UNIX_SECS] [--repo PATH] [--json]` reads
  `usage_log::read(default_path())` and prints `blyte $<o+i> (orchestrator $<o>, implementers $<i> in
  <runs> runs)` with two decimals, or the `Totals` as JSON with `--json`. Exit 0 even when the log is
  missing (totals are zero).
- REPL: after each reply the usage line gains ` · session $<o+i> (implementers $<i>, <runs> runs)`
  computed with `totals(..., Some(session_id), None, None)`; `/cost` prints the same totals line as
  `blyte cost --session <id>`.

**Tests:** `totals` with a mix of entries (two sessions, dispatch and not, one with `cost_usd: None`,
timestamps either side of `since`, two repos), checking each filter alone and combined; the printed
line's format for a known `Totals`; `/cost` output in a REPL test with `BLYTE_USAGE_LOG` pointing at a
temp file holding two entries.
Run: `cargo test -p blyte cost` and `cargo test -p blyte repl`.

**Depends on:** U1, U2.

**Commit message:** `feat(usage): blyte cost and the session's combined cost in the REPL`

## Worktree setup (2026-10-07)

From the `provision` deletion job on the EC2 box `provision-deployment`: its gate runs
`.venv/bin/python -m pytest ...`, a fresh worktree has no `.venv`, and del-T1 spent about 70 turns
(6 minutes) failing to build one before it was redone; del-T2 passed only because the main
checkout's `.venv` was symlinked into its worktree by hand. blyte has no step that prepares a
worktree's environment. Decision: an optional `setup` command in `blyte.toml`, run once by
`blyte dispatch` in the new worktree before the implementer starts. It is the repository owner's
command (like the gate), run outside the sandbox so it can reach package caches and indexes; a
failure ends the run as `blocked` without spending a turn.

### Task S1: `[dispatch].setup` runs before the implementer

**Goal:** prepare a dispatch worktree's environment (a virtualenv, `node_modules`) with a command from
`blyte.toml`, so the implementer never spends turns on it, and a broken environment is reported as
`blocked` at once.

**Files:** `crates/blyte-cli/src/config.rs`, new `crates/blyte-cli/src/setup.rs` (and `pub mod setup;`
in `crates/blyte-cli/src/lib.rs`), `crates/blyte-cli/src/dispatch.rs`, `docs/usage.md`, new
`crates/blyte-cli/tests/test_dispatch_setup.rs`.

**What to build:**
- `DispatchConfig` gains `pub setup: Option<String>`. The commented template in `config.rs` (the
  `# fold_budget = 60000` block) gains `# setup = "uv sync"   # prepares each new worktree before the
  implementer starts`.
- `setup.rs`:
  ```rust
  pub const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

  /// Runs `setup` with bash in `worktree`, outside the sandbox, writing its combined output to
  /// `log_path`. Every `env` entry and `BLYTE_MAIN_CHECKOUT=<main_checkout>` are exported first.
  /// Ok(()) on exit 0; Err(message) otherwise, where message is
  /// "setup failed (exit N)" or "setup failed: <error>" (a timeout or spawn failure), followed by
  /// a newline and the last 20 lines of the output.
  pub fn run_setup(setup: &str, worktree: &Path, main_checkout: &Path,
                   env: &BTreeMap<String, String>, log_path: &Path,
                   timeout: Duration) -> Result<(), String>
  ```
  Run it through `blyte_agent::shell::LocalShell::new().exec(script, worktree, timeout)` (as
  `init.rs` `verify_steps` does). That shell removes `BLYTE_*` variables from the environment, so
  pass the variables as `export NAME='value'` lines at the top of the script, quoting each value in
  single quotes with `'` written as `'\''`. Write the log even when the command fails.
- `dispatch.rs` `run_dispatch`: after step 3 (base sha) and before writing the task file and spawning
  the child, when not `--research` and `dispatch_cfg.setup` is `Some`, print
  `setup: <first line of setup>` to stderr and call `run_setup` with the worktree, the main checkout
  (`config::main_checkout(&root)`, falling back to `root`), `dispatch_cfg.env`,
  `.wt/<id>.setup.log`, and `SETUP_TIMEOUT`. On `Err(msg)`: print and write `.wt/<id>.json` exactly
  as the normal path does, with outcome `blocked`, exit `exit_code("blocked")`, `error` =
  the first line of `msg`, `final_message` = `msg`, no commits or changed files, zero turns and gate
  attempts, `cost_usd` None, `log` = the setup log path, `report` = `""`; then return that exit code.
  Factor the summary printing and writing into one helper used by both paths rather than copying it.
  On `Ok`, continue as today. With no `setup`, nothing changes.
- `docs/usage.md`: a `setup` row in the `blyte.toml` key table (`string`, "a command run once in each
  new dispatch worktree before the implementer starts, outside the sandbox, with `[dispatch.env]` and
  `BLYTE_MAIN_CHECKOUT` set; a failure ends the run as `blocked`"), and a short paragraph with the two
  usual forms: `setup = "uv sync"` (a virtualenv per worktree; safe when tasks change dependencies) and
  `setup = "ln -s \"$BLYTE_MAIN_CHECKOUT/.venv\" .venv"` (share the main checkout's; instant, but
  runs that change dependencies affect each other).

**Tests** (`test_dispatch_setup.rs`, using temp dirs; set `BLYTE_USAGE_LOG=off` for any child process):
1. `load` reads `setup = "uv sync"` into `dispatch.setup`; a blyte.toml without it gives `None`.
2. `run_setup("echo hi > made.txt && echo \"$BLYTE_MAIN_CHECKOUT|$FOO\"", ...)` with env
   `FOO = "it's"` returns Ok, creates `made.txt` in the worktree, and the log contains
   `<main_checkout>|it's`.
3. `run_setup("echo broken; exit 3", ...)` returns Err starting with `setup failed (exit 3)` and
   containing `broken`; the log exists and contains `broken`.
4. Output of 30 lines (`seq 1 30; exit 1`): the Err holds lines 11..=30 and not line `10`.
5. A timeout (`sleep 5` with a 1 s timeout) returns Err starting with `setup failed:`.
6. End to end, through the binary (`env!("CARGO_BIN_EXE_blyte")`): a temp git repository with one
   commit and `blyte.toml` holding `gate = "true"` and `setup = "echo broken; exit 3"`; run
   `blyte dispatch S1x <task.md>` in it. It exits `5` (the `blocked` code), `.wt/S1x.json` has
   outcome `blocked` and a `final_message` containing `broken`, `.wt/S1x.setup.log` exists, and no
   `.wt/S1x.log` exists (the implementer never started). No API key or model is needed, since the run
   stops before the child is spawned.

Run: `cargo test -p blyte --test test_dispatch_setup`.

**Constraints:** existing tests pass unmodified. Do not change `init.rs` (Task S2).

**Depends on:** none.

**Commit message:** `feat(dispatch): run blyte.toml's setup in each new worktree before the implementer`

### Task S2: `blyte init` proposes a setup for uv repositories

**Goal:** a repository with `uv.lock` gets a working `setup` from `blyte init`, and init's own step
checks run after it, so a gate that uses `.venv/bin/...` is proved the way dispatch will run it.

**Files:** `crates/blyte-cli/src/init.rs`, `crates/blyte-cli/tests/test_init_verified.rs` (add tests).

**What to build:**
- `pub fn detect_setup(root: &Path) -> Option<String>`: `Some("uv sync")` when `root/uv.lock` exists,
  else `None` (`UV_FROZEN`, which `detect_env` already sets, makes it frozen).
- `verify_steps` takes `setup: Option<&str>` and, when given, runs it in the init worktree with
  `blyte_cli::setup::run_setup` (env, main checkout = `root`, log under the init worktree's parent as
  `init-<pid>.setup.log`, `SETUP_TIMEOUT`) before the steps. If setup fails, print
  `warning: setup failed; left out of blyte.toml:` and the message, and verify the steps without it.
  Return whether setup passed along with the steps (change the return type as needed).
- `render_verified` takes `setup: Option<&str>` plus whether it passed: passed renders
  `setup = "<setup>"` under `[dispatch]` before `gate`, with the comment line
  `# prepares each new worktree before the implementer starts`; failed renders it commented out
  (`# setup = "<setup>"   # left out: failed in a fresh worktree`); `None` renders neither.
- `run` passes `detect_setup(root)` through.

**Tests:** `detect_setup` with and without `uv.lock`; `render_verified` for each of the three cases,
and in each case the rendered text loads with `blyte_cli::config::load` (written to a temp
`blyte.toml`) with `dispatch.setup` equal to `Some("uv sync")` only when it passed.
Run: `cargo test -p blyte --test test_init_verified`.

**Constraints:** existing tests pass; update existing callers of `render_verified` and
`verify_steps` for the new parameters only.

**Depends on:** S1 (uses `setup::run_setup` and the `setup` field).

**Commit message:** `feat(init): propose uv sync as the worktree setup for uv repositories`

## A typed subcommand never starts an agent (2026-10-07)

On 2026-10-07 `blyte cost` was run with an installed binary older than the `cost` command; it took
`cost` as a one-shot prompt and started an agent in the blyte checkout. A new subcommand typed into an
older build must fail, not run a model.

### Task G1: refuse a one-word prompt

**Goal:** `blyte <word>` where `<word>` is a single token with no whitespace is an error, not a
one-shot prompt, so a subcommand the running build does not have (or a typo) never starts an agent.

**Files:** `crates/blyte-cli/src/main.rs`, new `crates/blyte-cli/tests/test_one_word_prompt.rs`.

**What to build:** in `main.rs`, where `cli.prompt` is `Some(prompt)` (before any provider, config or
model is resolved), if `prompt.trim()` contains no whitespace, print to stderr
`error: \`<word>\` is not a blyte command in this build (blyte <version>); a prompt needs more than one word`
using the same version string `blyte --version` prints, and exit 2. Prompts of two or more words are
unchanged.

**Tests** (through the binary, `env!("CARGO_BIN_EXE_blyte")`, in a temp dir, with `BLYTE_USAGE_LOG=off`
and no provider keys in the environment):
1. `blyte nosuchcommand` exits 2, stderr contains `` `nosuchcommand` is not a blyte command `` and the
   version, and finishes in under 5 seconds (no model call).
2. `blyte "  cost  "` behaves the same (trimmed).
3. A real subcommand still works: `blyte cost` exits 0.
4. `blyte "fix the bug"` is not refused by this check: its stderr does not contain `is not a blyte command`
   (it may fail later for a missing key; that is fine).

Run: `cargo test -p blyte --test test_one_word_prompt`.

**Constraints:** existing tests pass unmodified.

**Depends on:** none. **Siblings:** S2 changes `init.rs`; not here.

**Commit message:** `fix(cli): a one-word prompt is an error, so an unknown subcommand never starts an agent`

## Costs on the Claude Code status line (2026-10-07)

Dispatches started from Claude Code are logged with no session, so the status line cannot show what a
Claude session spent on implementers. Claude Code sets `CLAUDE_CODE_SESSION_ID` in its commands'
environment, and the same id reaches the status line script as `session_id`; the dispatch child
inherits it. The usage log falls back to it when `BLYTE_SESSION` is unset.

### Task C1: a usage entry's session falls back to Claude Code's session id

**Goal:** `blyte cost --session <claude session id>` totals the dispatches a Claude Code session started.

**Files:** `crates/blyte-cli/src/usage_log.rs`, `crates/blyte-cli/src/task_runner.rs`, new
`crates/blyte-cli/tests/test_usage_session.rs`.

**What to build:** in `usage_log.rs`,
`pub fn session_from(blyte: Option<String>, claude: Option<String>) -> Option<String>`: the first of the
two that is `Some` and non-empty after trimming, `blyte` first; otherwise `None`. `task_runner.rs` sets
the entry's `session` to
`session_from(std::env::var("BLYTE_SESSION").ok(), std::env::var("CLAUDE_CODE_SESSION_ID").ok())`.
Nothing else changes.

**Tests** (`test_usage_session.rs`): `session_from(Some("b"), Some("c")) == Some("b")`;
`session_from(None, Some("c")) == Some("c")`; `session_from(Some(""), Some("c")) == Some("c")`;
`session_from(Some("  "), None) == None`; `session_from(None, None) == None`.
Run: `cargo test -p blyte --test test_usage_session`.

**Constraints:** existing tests pass unmodified.

**Depends on:** none. **Siblings:** ST1 changes `repl.rs`; not here.

**Commit message:** `feat(usage): log a Claude Code session's dispatches under its session id`

### Task ST1: `/state` summarises the orchestrator instructions

**Goal:** `/state` in the REPL printed the whole system prompt (about 12k characters under
`--orchestrate`), burying the useful fields.

**Files:** `crates/blyte-cli/src/repl.rs`.

**What to build:** in the `"/state"` arm, before printing `session.state.orchestrator` as JSON, convert
it to a `serde_json::Value` and, if it is an object with a string field `instructions`, replace that
field's value with the string `"<N characters>"` where N is the original length in characters. Every
other field prints as today.

**Tests:** extend the existing `/state` test in `repl.rs` (the one calling
`handle_repl_command(&mut session, "/state", ...)`): set `session.state.orchestrator.instructions` to a
600-character string first, then assert the output contains `"<600 characters>"` and does not contain
the 600-character string.
Run: `cargo test -p blyte repl`.

**Constraints:** existing tests pass unmodified.

**Depends on:** none. **Siblings:** C1 changes `usage_log.rs` and `task_runner.rs`; not here.

**Commit message:** `fix(repl): /state shows the instructions' length, not the whole prompt`

## Read access from blyte.toml (2026-10-07)

On `provision-deployment` the setup `ln -sfn /home/ubuntu/repos/provision/.venv .venv` works, but the
sandboxed implementer and gate then read the main checkout's `.venv`, which needs
`--allow-read /home/ubuntu/repos/provision/.venv` on every dispatch. `blyte.toml` has no key for it,
and an unknown key fails to parse (`deny_unknown_fields`), so it cannot be set there.

### Task A1: `[dispatch].allow_read` in blyte.toml

**Goal:** a repository can give its dispatches extra read access (such as a shared virtualenv) in
`blyte.toml`, not only with `--allow-read` on each dispatch.

**Files:** `crates/blyte-cli/src/config.rs`, `crates/blyte-cli/src/dispatch.rs`, `docs/usage.md`, new
`crates/blyte-cli/tests/test_config_allow_read.rs`.

**What to build:**
- `DispatchConfig` gains `#[serde(default)] pub allow_read: Vec<String>`. The commented template in
  `config.rs` gains `# allow_read = ["~/.cache/some-tool"]   # extra paths the sandbox may read`.
- `pub fn expand_allow_read(entries: &[String], home: Option<&Path>, main_checkout: &Path) -> Result<Vec<PathBuf>, String>`
  in `config.rs`: `~` or a leading `~/` is the home directory, a leading `$BLYTE_MAIN_CHECKOUT` (alone
  or followed by `/`) is the main checkout; any other entry must be an absolute path. A relative path,
  or `~` with no home, is an `Err` naming the entry and `blyte.toml`.
- `load` calls `expand_allow_read` with `dirs`/`HOME` (whatever `config.rs` or the crate already uses
  for the home directory; `std::env::var_os("HOME")` if nothing) and `main_checkout(root)` falling back
  to `root` to validate the entries, and returns its error, so a bad entry fails the load with a clear
  message; `dispatch.allow_read` keeps the entries as written.
- `dispatch.rs`: expand `dispatch_cfg.allow_read` the same way, then give the child
  `--allow-read <path>` for every expanded entry from
  `blyte.toml` followed by every `--allow-read` given on the command line (both apply; no
  de-duplication needed).
- `docs/usage.md`: an `allow_read` row in the key table (`array of strings`, "extra paths the sandbox may
  read, for every dispatch; `~/` and `$BLYTE_MAIN_CHECKOUT/` are expanded, otherwise absolute"), and in
  the `setup` paragraph, after the `ln -s` form, add: "The sandbox must then be allowed to read the
  shared virtualenv: `allow_read = [\"$BLYTE_MAIN_CHECKOUT/.venv\"]`."

**Tests** (`test_config_allow_read.rs`):
1. `expand_allow_read(["~/x", "~", "$BLYTE_MAIN_CHECKOUT/.venv", "$BLYTE_MAIN_CHECKOUT", "/abs/p"], Some("/h"), "/m")`
   gives `/h/x`, `/h`, `/m/.venv`, `/m`, `/abs/p` in order.
2. `"rel/p"` is an `Err` containing `rel/p`; `"~/x"` with `home: None` is an `Err`.
3. `load` of a temp `blyte.toml` with `allow_read = ["/abs/p"]` succeeds and `dispatch.allow_read`
   holds it; with `allow_read = ["rel"]` it fails with a message containing `rel` and `blyte.toml`;
   a file without the key gives an empty list.

Run: `cargo test -p blyte --test test_config_allow_read`.

**Constraints:** existing tests pass unmodified.

**Depends on:** none. **Siblings:** C1 (`usage_log.rs`, `task_runner.rs`) and ST1 (`repl.rs`) run
separately; not here.

**Commit message:** `feat(config): allow_read in blyte.toml for paths every dispatch may read`

## blyte on the Claude Code status line (2026-10-07)

User decision: when blyte is installed for Claude Code, it puts its costs on Claude Code's status line.
`blyte statusline` prints the line; `blyte skill install` (which `blyte update` re-runs) sets it in
`~/.claude/settings.json`. A status line the user already has is kept and wrapped
(`blyte statusline --after '<their command>'`), not replaced; uninstall restores it. The line's blyte
part needs C1 (dispatches logged under the Claude session id).

### Task SL1: `blyte statusline`

**Goal:** a status line command for Claude Code that shows the session's Claude cost and what its blyte
dispatches cost.

**Files:** new `crates/blyte-cli/src/statusline.rs`, `crates/blyte-cli/src/lib.rs` (`pub mod statusline;`),
`crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`, new `crates/blyte-cli/tests/test_statusline.rs`.

**What to build:**
- `blyte statusline [--after <COMMAND>]`: reads Claude Code's status JSON from stdin, prints one line,
  always exits 0 (any error degrades to a shorter line, never a panic or a non-zero exit).
- `pub fn base_line(d: &serde_json::Value, compact_window: Option<u64>) -> String`, built from the parts
  that are present, joined with ` · `:
  - `d.model.display_name`, or `claude` when missing.
  - context: with `p = d.context_window.used_percentage` and `size = d.context_window.context_window_size`
    (or `.size`): `used = p / 100 * size`, `limit = min(size, compact_window.unwrap_or(200000))`,
    text `context {used/1000:.0}k of {limit/1000}k`, plus ` — consider /compact or a new session` when
    `100 * used / limit >= 60`. With `p` but no size: `context {p:.0}%`. Without `p`: no part.
  - `${d.cost.total_cost_usd:.2} this session` when present.
- `compact_window` comes from `autoCompactWindow` in `~/.claude/settings.json` (`None` if missing or
  unreadable).
- `pub fn blyte_part(totals: &crate::cost::Totals, claude_cost: Option<f64>) -> Option<String>`: `None`
  when `totals.runs == 0`; otherwise `blyte ${implementers_usd:.2} ({runs} runs)`, followed by
  ` · total ${claude_cost + implementers_usd:.2}` when `claude_cost` is `Some`.
- The totals are `crate::cost::totals(&usage_log::read(default_path()), Some(d.session_id), None, None)`;
  no `session_id` in the input means no blyte part.
- With `--after <COMMAND>`: run `sh -c <COMMAND>` with the same stdin bytes, a 2-second timeout and
  stderr discarded; the base is the first line of its stdout, trimmed. If it fails, times out or
  prints nothing, use `base_line` instead.
- Output: the base, then ` · ` and the blyte part when there is one.

**Tests** (`test_statusline.rs`):
1. `base_line` for full input (`used_percentage: 45`, `context_window_size: 200000`, cost `1.5`, model
   `Opus`) with `compact_window: Some(200000)` gives `Opus · context 90k of 200k · $1.50 this session`.
2. 65% of 200k gives the `consider /compact` suffix; percentage without size gives `context 45%`;
   `{}` gives `claude`.
3. `blyte_part` with 0 runs is `None`; with 2 runs and $0.07 and claude cost 2.5 it is
   `blyte $0.07 (2 runs) · total $2.57`; with claude cost `None` there is no total.
4. Through the binary with `BLYTE_USAGE_LOG` pointing at a temp file holding two entries for session `s1`
   (one with `dispatch: Some`), stdin `{"session_id":"s1","model":{"display_name":"Opus"},"cost":{"total_cost_usd":1}}`:
   the output contains `Opus` and `blyte $` and `(1 runs)`, and the exit is 0.
5. `--after "echo mine"` prints a line starting with `mine`; `--after "exit 1"` falls back to the base
   line; `--after "sleep 5"` falls back within 4 seconds.
6. Malformed stdin (`not json`) prints `claude` and exits 0.

Run: `cargo test -p blyte --test test_statusline`.

**Constraints:** existing tests pass unmodified.

**Depends on:** none. **Siblings:** SL2 changes `skill.rs`; A1 changes `config.rs`, `dispatch.rs`;
C1 changes `usage_log.rs`, `task_runner.rs`; ST1 changes `repl.rs`. None are in this worktree.

**Commit message:** `feat(statusline): blyte statusline, Claude Code's line with blyte's costs`

### Task SL2: `blyte skill install` sets the Claude Code status line

**Goal:** installing blyte for Claude Code puts `blyte statusline` on Claude Code's status line,
keeping a status line the user already has.

**Files:** `crates/blyte-cli/src/skill.rs`, `crates/blyte-cli/src/cli.rs` (the `--no-statusline` flag on
`skill install` only), `crates/blyte-cli/src/main.rs` (pass it through), `Cargo.toml` (serde_json
`preserve_order` feature), new `crates/blyte-cli/tests/test_skill_statusline.rs`.

**What to build:**
- Enable serde_json's `preserve_order` feature in the workspace `Cargo.toml`, so rewriting
  `settings.json` keeps the user's key order.
- In `skill.rs`, pure functions on the settings file's text (an empty string means `{}`):
  - `pub fn set_status_line(settings: &str) -> Result<Option<String>, String>`: `Ok(None)` (no change)
    when `statusLine.command` already starts with `blyte statusline`. Otherwise it returns the new text
    with `statusLine` set to `{"type":"command","command":C}`, where C is `blyte statusline` when there
    was no `statusLine` (or it had no command) and `blyte statusline --after '<old>'` when there was a
    command `<old>`, with `'` in `<old>` written as `'\''`. Other keys and their order are kept; output
    is pretty-printed with two-space indent and a trailing newline. Invalid JSON is an `Err` (the file
    is then left alone).
  - `pub fn remove_status_line(settings: &str) -> Result<Option<String>, String>`: if the command is
    exactly `blyte statusline`, remove `statusLine`; if it is `blyte statusline --after '<old>'`, set
    the command back to `<old>` (undoing the quoting); otherwise `Ok(None)`.
- `install_for` with `Harness::Claude` and not `project` and not `no_statusline`: apply
  `set_status_line` to `~/.claude/settings.json` (create it if missing). Before the first change, copy
  the existing file to `~/.claude/settings.json.blyte-backup` if no backup exists. Print
  `status line: set to blyte statusline` (or `... wrapped your status line command`) when it changes. A
  JSON error prints a warning naming the file and does not fail the install.
- `uninstall_for` with `Harness::Claude` and not `project`: apply `remove_status_line`.
- `skill install` gains `--no-statusline` ("do not set Claude Code's status line").

**Tests** (`test_skill_statusline.rs`, pure functions only, no real home directory):
1. `set_status_line("")` and `set_status_line("{}")` give `statusLine.command == "blyte statusline"`.
2. `{"model":"opus","statusLine":{"type":"command","command":"/u/it's.py"},"z":1}` gives
   `blyte statusline --after '/u/it'\''s.py'`, and the keys come out in the order `model`, `statusLine`, `z`.
3. Running `set_status_line` again on its own output is `Ok(None)`.
4. `remove_status_line` of (1)'s output has no `statusLine`; of (2)'s output gives back command
   `/u/it's.py`; of `{"statusLine":{"command":"mine"}}` is `Ok(None)`.
5. `set_status_line("{not json")` is `Err`.

Run: `cargo test -p blyte --test test_skill_statusline`.

**Constraints:** existing tests pass unmodified (if enabling `preserve_order` breaks one that compares
JSON text, say so in your final message and fix that test's expectation only). Do not add the
`statusline` command itself (SL1).

**Depends on:** none (it writes the command's name only). **Siblings:** SL1 adds `blyte statusline`
and touches `cli.rs`/`main.rs` too; keep your `cli.rs`/`main.rs` edits to the one flag so the merge is
small. A1, C1 and ST1 are separate as well.

**Commit message:** `feat(skill): installing for Claude Code sets the status line to blyte statusline`
