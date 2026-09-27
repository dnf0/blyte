# Getting started

blyte is meant to sit under a frontier model: Claude Code (or another frontier harness) plans and
reviews, and blyte implements each task on a cheap model. This page sets that up. Running blyte
on its own is covered at the end.

## 1. Install blyte

blyte is a single Rust binary, for macOS and Linux. With a recent stable Rust toolchain:

```bash
cargo install --git https://github.com/climate-x-org/superstate blyte --locked
blyte --version        # blyte 0.1.0 (<commit>)
```

## 2. Give it a model

blyte's model does the implementation, so pick a cheap one that follows instructions well. The
default, and the one blyte has been measured on, is Gemini 3.8 Flash:

```bash
export GEMINI_API_KEY=...
```

Other providers (Anthropic, any OpenAI-compatible API, OpenRouter, Ollama, a ChatGPT
subscription) are on [Providers](providers.md). blyte reads the key itself; it never reaches the
commands the model runs.

## 3. Connect Claude Code

The blyte repository ships a Claude Code skill and two small scripts that implement the
workflow. Install them as a personal skill:

```bash
git clone https://github.com/climate-x-org/superstate.git blyte-src
mkdir -p ~/.claude/skills/blyte-implementer
cp blyte-src/examples/orchestrate/claude-skill/blyte-implementer/SKILL.md \
   blyte-src/examples/orchestrate/blyte-*.sh ~/.claude/skills/blyte-implementer/
```

In the repository you want to work on, add `.wt/` to `.gitignore` (blyte's worktrees live there),
and make sure you have a check command that proves a change is right: your tests, plus
formatting and lint.

## 4. Delegate your first task

Start Claude Code in the repository and ask for the pattern explicitly:

> Plan this change, then use blyte to implement each task and review the results: *make
> `parse()` return an error on empty input instead of panicking.*

Claude will:

1. write the task to a file, with the tests that prove it and the exact commit message;
2. dispatch it: blyte works in `.wt/T1` on branch `wip/T1`, with your check command as its gate;
3. read blyte's summary (outcome, gate failures, turns, cost) and review the diff itself;
4. merge it, or send a follow-up task.

Your Claude usage goes on steps 1, 3 and 4. [blyte as your implementer](orchestration.md)
explains the workflow in full, and [Writing tasks for blyte](writing-tasks.md) what makes a task
succeed first time.

## Running blyte on its own

blyte also runs directly, from the root of a git repository:

```bash
blyte "handle empty input in parse() and add a test" --gate "cargo test"
```

It works until the gate exits 0 or a budget runs out:

| exit | meaning |
|---|---|
| `0` | verified: the gate passed (ungated: the model said it was finished) |
| `1` | the gate failed, a budget ran out, or an error stopped the run |

!!! warning "Start from a clean tree"
    By default a failed gate resets the tree to the commit the run started from, discarding
    uncommitted changes (`--rollback=false` keeps them). The orchestration scripts turn rollback
    off and run in a separate worktree, so your checkout is never touched.

A gate of several lines is several steps; every step runs and every failure comes back at once:

```bash
blyte "..." --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

`blyte` with no prompt starts a REPL (`/help`, `/model`, `/gate`, `/state`, `/diff`,
`/workspace`, `/clear`, `/exit`). Every flag is on [Running tasks](usage.md) and
[CLI flags](reference/cli.md).
