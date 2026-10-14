# Getting started

blyte is meant to sit under a frontier model: Claude Code (or another frontier harness) plans and
reviews, and blyte implements each task on a cheap model. This page sets that up. Running blyte
on its own is covered at the end.

## 1. Install blyte

blyte is a single Rust binary, for macOS and Linux. With a recent stable Rust toolchain:

```bash
cargo install blyte --locked
blyte --version        # blyte <version> (<commit>)
```

Or, without Rust, download the ready-built binary from the latest release with the GitHub CLI
(`blyte-x86_64-apple-darwin` on an Intel Mac, `blyte-x86_64-unknown-linux-musl` on Linux):

```bash
gh release download --repo dnf0/blyte --pattern blyte-aarch64-apple-darwin -O ~/.local/bin/blyte
chmod +x ~/.local/bin/blyte
```

To update later, `blyte update` installs the latest release in place (`--check` only shows the
versions; `--rollback` restores the previous build, kept as `blyte.prev`).

## 2. Give it a model

blyte's model does the implementation, so pick one that follows instructions well. `blyte skill
install` asks for your default model (`gemini-4-argon` recommended, `gemini-3.8-flash-high` built-in
default, or any model id) and writes `BLYTE_MODEL` to `~/.config/blyte/env` (or pass `blyte skill
install --model <id>`):

```bash
export GEMINI_API_KEY=...
```

A session that did not start with the key exported (a Claude Code session opened from another
shell, say) will not see it. So you can keep it in `~/.config/blyte/env`, which blyte reads for any
variable not already set:

```bash
mkdir -p ~/.config/blyte && printf 'GEMINI_API_KEY=%s\n' "$GEMINI_API_KEY" > ~/.config/blyte/env
chmod 600 ~/.config/blyte/env
```

blyte refuses the file if other users can read it. `BLYTE_ENV_FILE` names another file; an empty
`BLYTE_ENV_FILE` turns it off.

Other providers (Anthropic, any OpenAI-compatible API, OpenRouter, Ollama, a ChatGPT
subscription) are on [Providers](providers.md). blyte reads the key itself; it never reaches the
commands the model runs.

## 3. Connect Claude Code

blyte carries a Claude Code skill that teaches Claude the workflow. Install it for every project:

```bash
blyte skill install
```

or for one repository only (committed with it, so everyone on the repository gets it):

```bash
cd your-repo && blyte skill install --project
```

In an interactive terminal, `blyte skill install` asks for the default implementer model when
`BLYTE_MODEL` is not yet in `~/.config/blyte/env`; pass `--model <id>` to set `BLYTE_MODEL` without
asking. Installing also adds a short, marked block to `CLAUDE.md` (`~/.claude/CLAUDE.md`, or the
repository's with `--project`) that makes blyte Claude Code's default implementer, with
independent tasks dispatched in parallel. Pass `--no-default` to skip it; `blyte skill uninstall`
removes both. The skill always matches the installed blyte; after upgrading blyte, run
`blyte skill install --force` to refresh it. Make sure the repository has a check command that
proves a change is right: your tests, plus formatting and lint.

## 4. Delegate your first task

Start Claude Code in the repository and ask for a change as usual, for example *"make `parse()`
return an error on empty input, and add `--strict` to the CLI"*. Claude will:

1. split it into tasks, independent ones touching disjoint files, and write each to a file with
   the tests that prove it and the exact commit message;
2. run `blyte dispatch` for each, independent tasks at the same time: blyte works in `.wt/T1`,
   `.wt/T2`, … with your check command as the gate;
3. read the JSON summary (outcome, commits, blyte's own report, gate failures, cost) and review
   the diff with `blyte review`;
4. merge it, or send a follow-up task.

To watch the dispatches while they run, start `blyte dashboard --open` in another terminal
([Dashboard](usage.md#dashboard)).

Your Claude usage goes on steps 1, 3 and 4. [blyte as your implementer](orchestration.md)
explains the workflow in full, and [How tasks are written](writing-tasks.md) what makes a task
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
    uncommitted changes (`--rollback=false` keeps them). `blyte dispatch` turns rollback off and
    works in a separate worktree, so your checkout is never touched.

A gate of several lines is several steps; every step runs and every failure comes back at once:

```bash
blyte "..." --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

`blyte` with no prompt starts a REPL (`/help`, `/model`, `/gate`, `/state`, `/diff`,
`/workspace`, `/clear`, `/exit`). Every flag is on [Running tasks](usage.md) and
[CLI flags](reference/cli.md).
