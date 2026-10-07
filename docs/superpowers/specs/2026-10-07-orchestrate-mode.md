# `blyte --orchestrate`: blyte as the orchestrator

## Goal

Two interchangeable setups, chosen by which program the user starts:

- **Claude Code + blyte** (today): Claude Code follows the blyte-implementer skill and dispatches.
- **blyte + blyte**: `blyte --orchestrate -m vertex/gemini-4-argon` follows the same skill, runs the same
  commands (`blyte dispatch`, `blyte review`, `git merge --ff-only`), and dispatches implementers on the
  model in `blyte.toml` (`[dispatch].model`, gemini-3.8-flash).

There is no extra orchestration layer: the orchestrator is the existing interactive mode with different
defaults, and it drives blyte through the shell exactly as Claude Code does.

## What stops it today (from research OR0, 2026-10-07, checked against the code)

| Gap | Where | Effect |
|---|---|---|
| The orchestrator's shell is sandboxed; `blyte dispatch` then runs `sandbox-exec` inside it, which macOS refuses | `repl.rs:499`, `shell.rs:332` | dispatch fails |
| No orchestration instructions: the prompt says "autonomous software engineering agent"; the skill text (`skill.rs:4`, `SKILL_MD`) is never loaded | `run.rs:303-345` | it implements instead of dispatching |
| Replies are cut to 80 characters on screen | `repl.rs:82-90` | the user cannot read plans or questions |
| Shell commands time out at 300 s; a dispatch takes 10-30 min | `tools.rs:270` | foreground dispatch or `wait` fails |
| 60 turns, context folded at 48k tokens | `repl.rs:23,25`, `context.rs:59` | long sessions cut short or forget task details |
| After each reply, "Task verified in N turns"; with a gate set, the gate runs on the main checkout | `repl.rs:512-518`, `run.rs:805` | noise / wasted runs |

Already fine: the implementer model is independent of `-m`; API keys reach `blyte dispatch` through
`~/.config/blyte/env` (the shell strips provider keys from the environment, `shell.rs:136-160`, so keys
exported only in the terminal would not reach it; documented, not changed).

## Decisions

1. **One flag, `--orchestrate`**, on interactive `blyte`. It sets every default below; individual flags
   (`--max-turns`, `--context`) still override.
2. **The orchestrator's shell is not sandboxed** (user decision, 2026-10-07): the same trust Claude Code
   has. Every implementer it dispatches is still sandboxed by `blyte dispatch`. The orchestrator's network
   stays on.
3. **Instructions**, in this order in the system prompt, replacing the "autonomous software engineering agent"
   framing:
   - a short orchestrator preamble (you plan and review; blyte dispatches implement; never edit code
     yourself except one-line fixes; when you need a decision, end your reply with the question and wait);
   - the blyte-implementer skill (`SKILL_MD`, the same text Claude Code gets);
   - the user's own orchestrator rules from `~/.config/blyte/orchestrator.md` if present (the analogue of
     `~/.claude/CLAUDE.md`; capped like repository instructions);
   - repository instructions as today (`AGENTS.md`/`CLAUDE.md`).
4. **Waiting for dispatches: a new `blyte wait <ID>... [--any] [--timeout <secs>]`.** It returns when all
   (or, with `--any`, the first) of the runs have a summary in `.wt/<id>.json` or are dead, printing one line
   per finished run (`<id> <outcome> <turns> <cost>`), or exits 2 at the timeout (default 280 s, under the
   300 s command limit). The orchestrator starts dispatches with `&` and calls `blyte wait --any` in a loop:
   about one turn per five minutes, not per poll. The skill's "Starting parallel tasks" section gains a
   paragraph for this harness. The command timeout itself is unchanged.
5. **Limits:** 1000 turns; context `verbatim` (Argon has a 1M window; folding loses task text). No wall-clock
   limit (as today).
6. **No gate in orchestrate mode**; `--gate` with `--orchestrate` is an error. The "Task verified in N
   turns" line is replaced by the usage line only.
7. **Full replies everywhere**: the 80-character cut is removed for all interactive use, not only
   orchestration (it is a bug).

## Not in this change

Token streaming, Ctrl+C cancelling a turn, multi-line input, saving and resuming a session. Each degrades
the experience; none blocks the setup. Multi-line input is the first follow-up (pasting a plan is common).

## Tasks (plan to follow once agreed)

- **OC1** full replies (`repl.rs` `format_event`). Depends on: none.
- **OC2** `blyte wait` (new `wait.rs`, its registration in `cli.rs`/`main.rs`, tests). Depends on: none.
- **OC3** `--orchestrate` (`cli.rs`, `repl.rs`, `run.rs` prompt, `rules.rs` user file, skill paragraph,
  `docs/cli.md`). Depends on: OC1 (same file), OC2 (same `cli.rs`, and its prompt names `blyte wait`).

## Check before merging

With Argon not yet allowlisted, run OC3 for real with `-m vertex/gemini-3.8-flash` (or
`gemini-3.8-flash-high`) on a throwaway two-task plan in a scratch repository: it must dispatch both in
parallel, wait, review, record verdicts and merge, without the user typing commands.
