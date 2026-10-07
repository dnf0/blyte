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
