# The orchestrator's instructions: what blyte says, and what the user decides

Date: 2026-11-21. Status: agreed in outline; the trial in "Measuring the corrections" decides the last part.

## The problem

When blyte runs as an orchestrator (`blyte --orchestrate`, and the surface), the model is given text
the user did not write: a preamble, the whole `blyte-implementer` skill, tool descriptions and
reminders sent while it runs. An audit on 2026-11-21 (`main` at `25f3871`) found 58 separate
instructions of three kinds:

| Kind | Count | What it is | Evidence |
|---|---|---|---|
| Tool fact | 11 | True of blyte whatever the user's process: what a command does, what an outcome means | Mostly none needed; one is wrong |
| Correction | 26 | Added after a model wasted turns or did damage | Mostly measured, on Gemini 4 Argon and Gemini 3.8 Flash; none on the current default, `gemini-4-neon` |
| Process | 21 | How to plan, split, review, merge and report | Almost none; one measurement (2026-09-28) with a different model as the planner |

Eleven have no reason recorded anywhere. They were added one at a time and never read as a whole, so
they contradict each other:

- **Who writes the tests.** The preamble says the implementer does; the skill says the orchestrator
  writes them out in each task.
- **How much to prepare.** The preamble and a reminder after eight tool calls say to dispatch after a
  few reads; the skill says the plan is where the effort pays most.
- **Splitting work.** The preamble says to split work into several dispatches; `docs/evidence.md`
  measured that splitting one feature cost 30–160% more and was slower.
- **How to start on a plan.** One sentence says to run `blyte plan <file>` first; a leftover sentence
  says to read the plan and dispatch.
- **How to dispatch.** With the `dispatch` tool configured, one sentence says not to run
  `blyte dispatch` in the shell while the skill, sent in the same prompt, says to.
- **Background commands.** The `bash` tool's description says there is no background mode; the skill
  says to start dispatches in the background.
- **Asking the user.** The preamble says to end with a question and wait for the answer; in a
  one-shot run nobody answers and stopping ends the run.
- **Rules written for someone else.** The repository's implementer rules (`.blyte/rules.md`,
  `.claude/agy-rules.md`, `AGENTS.md`) are given to the orchestrator unchanged, with a sentence about
  a completion check it does not have.

The user can see none of this. No command prints the instructions, `/state` hides them, and nothing
says whether `~/.config/blyte/orchestrator.md` was loaded. That file is added after the built-in
text, so it can add to the process but not replace it.

## Decisions

1. **blyte states facts about itself and nothing else.** The built-in text is a reference: the
   commands, what each outcome means, where a run's files are, what the shell can do. It does not say
   how to plan, how to split work, who writes tests, when to merge or what to report.
2. **The process is a file the user owns.** blyte ships a default process file. A user's own file
   replaces it whole; it is not appended. The default is a starting point, said to be so wherever it
   is shown.
3. **The default process is consistent, and takes the side that was measured.** Plan carefully; write
   the tests into each task; one dispatch per feature, and parallel dispatches only across
   independent tasks. The sentences on the other side, and the reminder to dispatch after eight tool
   calls, are removed.
4. **No correction is kept on trust.** Each is removed from the built-in text and comes back only if
   the trial below shows its mistake on `gemini-4-neon`. One that comes back is recorded with the run
   that justified it.
5. **The orchestrator has its own repository file.** It no longer receives the implementer's rules.
6. **The user can see what is in force.** Every start says which process file and which repository
   file are used, and one command prints the full instructions.

## Design

### What the orchestrator is given

In this order:

1. **Reference** (built in, `crates/blyte-cli/assets/orchestrator/reference.md`). Facts only. It
   describes one way to dispatch, the one that is configured: the `dispatch` tool when
   `--dispatch-cmd` is set, otherwise `blyte dispatch` in the shell. It says whether a person will
   answer: in a conversation or the surface, a reply ending in a question waits for the user; in a
   one-shot run it says nobody will answer and the run ends when the model stops.
2. **Process.** The user's file if there is one, otherwise the default
   (`crates/blyte-cli/assets/orchestrator/process.md`), under a heading that names its source.
   The default is written for dispatching with `blyte dispatch`. With the `dispatch` tool configured
   there is no default process: the user's file is used if there is one, and the start line says
   `process: none` otherwise.
3. **Repository rules for the orchestrator**, if the repository has `.blyte/orchestrator.md`, under a
   heading that names the file. Added to the process, not replacing it: a process is how a person
   works, and a repository's file holds what is particular to that repository.
4. The user's request, the environment, and the tool descriptions, as now.

The user's file stays at `$XDG_CONFIG_HOME/blyte/orchestrator.md`, or `~/.config/blyte/orchestrator.md`.
`CLAUDE.md` is still not read.

**No fallbacks.** A user or repository file that exists but is empty, unreadable, or longer than the
limit (12,000 characters today) stops the start with an error that names the file and the limit. It
is no longer cut silently.

**A change for existing users.** A user's `orchestrator.md` written as additions now replaces the
default process. The release note says so, and the start line makes it visible.

### What is removed

- `PREAMBLE` and `DISPATCH_TOOL_INSTRUCTION` in `crates/blyte-agent/src/orchestrator.rs`: their tool
  facts move to the reference, their process sentences to the default process (where decision 3
  keeps them), their corrections out.
- `ORCHESTRATOR_EXPLORE_NUDGE` and its reminder in `crates/blyte-agent/src/run.rs`.
- The implementer's repository rules from the orchestrator's prompt.
- The skill as an injected block. `crates/blyte-cli/assets/blyte-implementer/SKILL.md` stays as the
  skill installed into other harnesses; the reference and the default process are written from it,
  and a test fails when either names a `blyte` command or flag the command line does not have.

"Change nothing outside the repository yourself" stays built in: it is a boundary, not a way of
working. Reminders the orchestrator shares with the implementer (turns left, time left, read volume,
repeated reads, cut-off replies, folded context) protect the run and stay as they are.

### What is corrected

The `bash` tool's description says there is no background mode. Before planning, check with a test
what the orchestrator's shell session does with a background job after its command returns, and make
the reference and the tool description say that one thing.

### What the user sees

- **At start**, in the terminal and on the surface: `process: default (built in)` or
  `process: <path>`, and `repository rules: <path>` or `repository rules: none`.
- **`blyte instructions`** prints the text the orchestrator would be given in the current
  repository, each part under its source, with the tool descriptions. The request and the
  description of the environment are added when a run starts and are not printed. `blyte instructions --default-process` prints the default
  process file, to copy and edit.
- The default process file begins by saying it is blyte's default, that it is a starting point, and
  how to replace it.

The surface's start line waits for the surface itself to merge (PR #377); the rest does not.

## Measuring the corrections

The corrections are removed first, then tested, because with the process in a file the test needs no
switch: a process file with the corrections appended is the second arm.

- **Task:** one real change in this repository with a gate, chosen before the runs, the same for
  every run, each from the same commit.
- **Arms:** A, the reference and the default process; B, the same with the removed corrections
  appended to the process file. Three one-shot runs of each on `gemini-4-neon`, each capped with
  `--max-cost 10` (USD 60 at most).
- **Counted from each run's log:** tool calls before the first dispatch; calls of each kind a
  correction names (`--help`, reading blyte's source, looking for API keys, reading `.wt/` beyond
  the named files, output not trimmed); turns; cost in USD; whether the combined result passes the
  gate; the reviewer's verdict.
- **Rule:** a correction returns when its mistake appears in at least two of the three A runs and
  not in B. A mistake in both arms means the sentence does not work and is dropped. Where the
  mistake can be stopped by the tool itself (an error message that says what to do), that is
  preferred to a sentence.
- **Limits:** three runs an arm shows only large effects, on one task and one model. The result is
  written to `docs/evidence.md` with those limits.

## Out of scope

- The implementer's own prompt and rules.
- Reading `CLAUDE.md` or other harnesses' files as the process.
- Different processes per model.
- Rebuilding `SKILL.md` from the two new files; the test above only keeps them from drifting.
