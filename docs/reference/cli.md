# CLI flags

The output of `blyte --help` and of each subcommand's `--help`. Behaviour is described in [Running tasks](../usage.md), [the workflow](../orchestration.md), [Providers](../providers.md) and [Security](../security.md).

```text
A lean coding-agent harness with a sealed verification gate

Usage: blyte [OPTIONS] [PROMPT] [COMMAND]

Commands:
  init       Detect this repository's checks and write blyte.toml
  dashboard  Serve a local dashboard of running and recent jobs
  dispatch   Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
  review     Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
  clean      Remove finished worktrees and merged branches
  skill      Install or print the Claude Code skill for the plan, delegate, review workflow
  update     Replace this binary with the latest release (the old one is kept as blyte.prev)
  help       Print this message or the help of the given subcommand(s)

Arguments:
  [PROMPT]  Direct task prompt to run in single-shot / headless mode

Options:
  -g, --gate <GATE>
          Verification gate command (e.g. 'pytest', 'cargo test'). Several steps may be given one per line: every step runs and all failures are reported together
      --context <CONTEXT>
          Context strategy: 'conversational' (append-all chat) or 'folded' (append-only chat, oldest half folded into digests past a budget) [default: folded] [possible values: conversational, folded]
  -m, --model <MODEL>
          Model identifier or provider selection
      --container <CONTAINER>
          Run inside an isolated container workspace instead of local host
      --max-turns <MAX_TURNS>
          Maximum turn budget [default: 25]
      --max-gates <MAX_GATES>
          Maximum verification gate attempts before failing [default: 5]
      --rollback[=<ROLLBACK>]
          Enable automatic rollback to baseline git commit if gate fails (disable with --rollback=false) [default: true] [possible values: true, false]
      --container-workdir <CONTAINER_WORKDIR>
          Working directory inside the container when --container is set [default: /workspace]
      --max-seconds <MAX_SECONDS>
          Wall-clock budget in seconds; the run stops itself and reports when it is spent
      --max-cost <MAX_COST_USD>
          Maximum spend in USD; the run stops before a model call once reached
      --report <REPORT>
          Write a JSON run report to this path when the headless run exits
      --fold-budget <FOLD_BUDGET>
          Approximate token budget for the verbatim tail before folding (folded context; default 48000)
      --sandbox <SANDBOX>
          Sandbox the commands blyte runs on this machine (macOS Seatbelt, Linux Landlock): 'on' or 'off' [default: on] [possible values: on, off]
      --network <NETWORK>
          Let commands blyte runs connect out: 'on' or 'off' (default 'on'). 'off' needs the sandbox [default: on] [possible values: on, off]
      --allow-read <ALLOW_READ>
          Let sandboxed commands read this path despite the credential deny-list (repeatable)
      --pass-env <PASS_ENV>
          Pass this environment variable to commands even though it looks like a credential (repeatable)
      --read-root <DIR>
          Directory allowed for reading with the read and grep tools (repeatable)
  -h, --help
          Print help
  -V, --version
          Print version
```

## blyte init

```text
Detect this repository's checks and write blyte.toml

Usage: blyte init [OPTIONS]

Options:
      --force                   Overwrite an existing blyte.toml
      --print                   Print the file instead of writing it
      --no-verify               Skip gate verification on HEAD
      --sandbox <SANDBOX>       Sandbox local command execution: 'on' or 'off' (default 'on') [default: on] [possible values: on, off]
      --step-timeout <SECONDS>  Timeout in seconds for each verification step (default 600) [default: 600]
  -h, --help                    Print help
```

## blyte dashboard

```text
Serve a local dashboard of running and recent jobs

Usage: blyte dashboard [OPTIONS]

Options:
      --port <N>        Port to listen on (0 for a free port) [default: 7171]
      --root <DIR>      Directory holding repositories or worktrees (repeatable)
      --runs-dir <DIR>  Directory holding run logs and reports (repeatable)
      --hours <N>       How far back finished jobs are listed, in hours [default: 24]
      --open            Open the dashboard in the default browser
  -h, --help            Print help
```

A job with no report counts as running only when its PID is alive and that process started no later than 60 seconds after the job's `started_at` (guarding against reused PIDs). In the runs-dir layout, `<id>.json` is recognized as an agy summary: jobs with this file report status `finished` (or `error`), with their elapsed duration, turn count, and response text.

## blyte dispatch

```text
Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary

Usage: blyte dispatch [OPTIONS] <ID> [TASK_FILE]

Arguments:
  <ID>         Identifier for this dispatch (used for worktree and branch wip/<id>)
  [TASK_FILE]  Path to the task description markdown file; optional with --plan and --plan-task, which take the task from the plan

Options:
  -g, --gate <GATE>                The check command; several lines are several steps, all of which must pass
      --tag <TEXT>                 Tag for this dispatch (repeatable)
      --plan <PATH>                Path to the plan file this dispatch belongs to
      --plan-task <TEXT>           Plan task this dispatch implements
      --base <BASE>                The commit a new worktree starts from [default: HEAD]
      --research                   Run read-only research: scratch repository, report gate, findings in report.md
      --report-to <PATH>           Copy the finished research report to this path (only with --research)
  -m, --model <MODEL>              Model identifier or provider selection
      --max-seconds <MAX_SECONDS>  Wall-clock budget in seconds (default 3600)
      --max-turns <MAX_TURNS>      Maximum turn budget (default 300)
      --max-cost <MAX_COST_USD>    Maximum spend in USD
      --fold-budget <FOLD_BUDGET>  Approximate token budget for the verbatim tail before folding
      --sandbox <SANDBOX>          Sandbox local command execution: 'on' or 'off' (default 'on') [default: on] [possible values: on, off]
      --network <NETWORK>          Outbound network mode: 'on' or 'off' (default 'on') [possible values: on, off]
      --allow-read <ALLOW_READ>    Let sandboxed commands read this path despite credential deny-list (repeatable)
      --pass-env <PASS_ENV>        Pass this environment variable to commands (repeatable)
  -h, --help                       Print help
```

The task file is optional when `--plan` and `--plan-task` are provided, which take the task directly from the plan.

When `blyte dispatch` uses the gate from `blyte.toml` (not an explicit `--gate` or `--research`), it checks whether check commands run by the repository's CI workflows (`.github/workflows/*.yml` and `*.yaml`) are missing from the gate.

Steps are matched by tool after stripping command wrappers (`env`, `xargs`, `uv run`, `uvx`, `poetry run`, `pipenv run`, `pdm run`, `npx`, `pnpm exec`, `bunx`, `python -m`). Commands are matched against check keys: `make <target>`, `cargo <subcmd>`, `go <subcmd>`, `npm run <script>` / `npm test`, `pnpm run <script>` / `pnpm test`, `yarn <script>`, `ruff <subcmd>`, or tool names (`pytest`, `pre-commit`, `pyright`, `mypy`, `eslint`, `vitest`, `jest`, `tsc`, `tox`, `nox`, `black`, `isort`). Equivalent npm, pnpm, and yarn scripts match one another.

A CI step counts as covered when:
1. A gate command has the same check key.
2. Any comment line in `blyte.toml` contains the step's check key text.
3. The tool's project file is missing at the repository root (`Cargo.toml`, `go.mod`, `package.json`, `Makefile`, `.pre-commit-config.yaml`, or Python project files).

The gate-drift warning never fails a dispatch. When uncovered steps exist, it prints a single line to stderr and continues normally:

```
blyte: the gate is missing CI steps: make typecheck (run blyte init --force to refresh blyte.toml)
```

## blyte review

```text
Show what a dispatched run did: commits, files, tree state, report, and optionally the diff

Usage: blyte review [OPTIONS] <ID>

Arguments:
  <ID>  

Options:
      --diff               
      --verdict <VERDICT>  [possible values: accepted, fixed, redone]
      --notes <TEXT>       
  -h, --help               Print help
```

## blyte clean

```text
Remove finished worktrees and merged branches

Usage: blyte clean [OPTIONS]

Options:
      --dry-run            Show what would be removed without deleting anything
      --older-than <DAYS>  Worktrees older than this many days are removable without a verdict (default 14) [default: 14]
      --all                Clean every repository in the registry
  -h, --help               Print help
```

A run's worktree is removed once its work is merged or discarded. `blyte clean` reports each worktree with a reason text:

Removable:
- `merged`: work is on the default branch (or `origin/<default>`).
- `merged into <base>`: work is on the branch the run was dispatched from.
- `merged through <child>`: work is merged through a follow-up run.
- `verdict redone`: the run was discarded with a `redone` verdict.
- `no changes`: the run completed with outcome `no_changes`.
- `research report`: a research run whose report was copied to `.wt/<id>.report.md`.
- `finished over N days ago`: the run finished more than the allowed age in days (default 14).

Kept:
- `not merged yet`: has an `accepted` or `fixed` verdict, but its work is not yet merged.
- `uncommitted changes`: the worktree has uncommitted modifications.
- `running`: the run's process is still alive.
- `no report`: a research run with no report file yet.

Other worktrees whose branch is merged and working directory is clean are reported with their `git worktree remove` commands, but never deleted automatically.

## blyte skill install

```text
A lean coding-agent harness with a sealed verification gate

Usage: blyte [OPTIONS] [PROMPT] [COMMAND]

Commands:
  dashboard  Serve a local dashboard of running and recent jobs
  dispatch   Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
  review     Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
  clean      Remove finished worktrees and merged branches
  skill      Install or print the Claude Code skill for the plan, delegate, review workflow
  update     Replace this binary with the latest release (the old one is kept as blyte.prev)
  help       Print this message or the help of the given subcommand(s)

Arguments:
  [PROMPT]  Direct task prompt to run in single-shot / headless mode

Options:
  -g, --gate <GATE>
          Verification gate command (e.g. 'pytest', 'cargo test'). Several steps may be given one per line: every step runs and all failures are reported together
      --context <CONTEXT>
          Context strategy: 'conversational' (append-all chat) or 'folded' (append-only chat, oldest half folded into digests past a budget) [default: folded] [possible values: conversational, folded]
  -m, --model <MODEL>
          Model identifier or provider selection
      --container <CONTAINER>
          Run inside an isolated container workspace instead of local host
      --max-turns <MAX_TURNS>
          Maximum turn budget [default: 25]
      --max-gates <MAX_GATES>
          Maximum verification gate attempts before failing [default: 5]
      --rollback[=<ROLLBACK>]
          Enable automatic rollback to baseline git commit if gate fails (disable with --rollback=false) [default: true] [possible values: true, false]
      --container-workdir <CONTAINER_WORKDIR>
          Working directory inside the container when --container is set [default: /workspace]
      --max-seconds <MAX_SECONDS>
          Wall-clock budget in seconds; the run stops itself and reports when it is spent
      --max-cost <MAX_COST_USD>
          Maximum spend in USD; the run stops before a model call once reached
      --report <REPORT>
          Write a JSON run report to this path when the headless run exits
      --fold-budget <FOLD_BUDGET>
          Approximate token budget for the verbatim tail before folding (folded context; default 48000)
      --sandbox <SANDBOX>
          Sandbox the commands blyte runs on this machine (macOS Seatbelt, Linux Landlock): 'on' or 'off' [default: on] [possible values: on, off]
      --network <NETWORK>
          Let commands blyte runs connect out: 'on' or 'off' (default 'on'). 'off' needs the sandbox [default: on] [possible values: on, off]
      --allow-read <ALLOW_READ>
          Let sandboxed commands read this path despite the credential deny-list (repeatable)
      --pass-env <PASS_ENV>
          Pass this environment variable to commands even though it looks like a credential (repeatable)
      --read-root <DIR>
          Directory allowed for reading with the read and grep tools (repeatable)
  -h, --help
          Print help
  -V, --version
          Print version
```

If an older, separate `blyte-research` skill directory exists (`~/.claude/skills/blyte-research`, `~/.codex/skills/blyte-research`, or `~/.agents/skills/blyte-research`), `blyte skill install` warns on stderr that it is superseded by the `blyte-implementer` skill's research section (default 300 turns) and should be removed (`rm -r <path>`).

## blyte skill uninstall

```text
A lean coding-agent harness with a sealed verification gate

Usage: blyte [OPTIONS] [PROMPT] [COMMAND]

Commands:
  dashboard  Serve a local dashboard of running and recent jobs
  dispatch   Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
  review     Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
  clean      Remove finished worktrees and merged branches
  skill      Install or print the Claude Code skill for the plan, delegate, review workflow
  update     Replace this binary with the latest release (the old one is kept as blyte.prev)
  help       Print this message or the help of the given subcommand(s)

Arguments:
  [PROMPT]  Direct task prompt to run in single-shot / headless mode

Options:
  -g, --gate <GATE>
          Verification gate command (e.g. 'pytest', 'cargo test'). Several steps may be given one per line: every step runs and all failures are reported together
      --context <CONTEXT>
          Context strategy: 'conversational' (append-all chat) or 'folded' (append-only chat, oldest half folded into digests past a budget) [default: folded] [possible values: conversational, folded]
  -m, --model <MODEL>
          Model identifier or provider selection
      --container <CONTAINER>
          Run inside an isolated container workspace instead of local host
      --max-turns <MAX_TURNS>
          Maximum turn budget [default: 25]
      --max-gates <MAX_GATES>
          Maximum verification gate attempts before failing [default: 5]
      --rollback[=<ROLLBACK>]
          Enable automatic rollback to baseline git commit if gate fails (disable with --rollback=false) [default: true] [possible values: true, false]
      --container-workdir <CONTAINER_WORKDIR>
          Working directory inside the container when --container is set [default: /workspace]
      --max-seconds <MAX_SECONDS>
          Wall-clock budget in seconds; the run stops itself and reports when it is spent
      --max-cost <MAX_COST_USD>
          Maximum spend in USD; the run stops before a model call once reached
      --report <REPORT>
          Write a JSON run report to this path when the headless run exits
      --fold-budget <FOLD_BUDGET>
          Approximate token budget for the verbatim tail before folding (folded context; default 48000)
      --sandbox <SANDBOX>
          Sandbox the commands blyte runs on this machine (macOS Seatbelt, Linux Landlock): 'on' or 'off' [default: on] [possible values: on, off]
      --network <NETWORK>
          Let commands blyte runs connect out: 'on' or 'off' (default 'on'). 'off' needs the sandbox [default: on] [possible values: on, off]
      --allow-read <ALLOW_READ>
          Let sandboxed commands read this path despite the credential deny-list (repeatable)
      --pass-env <PASS_ENV>
          Pass this environment variable to commands even though it looks like a credential (repeatable)
      --read-root <DIR>
          Directory allowed for reading with the read and grep tools (repeatable)
  -h, --help
          Print help
  -V, --version
          Print version
```

## blyte update

```text
Replace this binary with the latest release (the old one is kept as blyte.prev)

Usage: blyte update [OPTIONS]

Options:
      --check     Only show the installed and latest versions
      --rollback  Swap back to the previous build (blyte.prev)
  -h, --help      Print help
```

It downloads this machine's binary from the latest GitHub release through the GitHub CLI (`gh`,
signed in with access to the repository), checks that it runs, keeps the old binary beside it as
`blyte.prev`, and refreshes every unedited copy of the skill it installed (for Claude Code and Codex).
Like `blyte skill install`, it also checks for a superseded `blyte-research` skill directory and
warns on stderr to remove it.

## Environment

| variable | meaning |
|---|---|
| `BLYTE_MODEL` | default for `--model` |
| `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `OPENROUTER_API_KEY`, `OLLAMA_HOST`, `OLLAMA_API_KEY` | provider credentials and endpoints; never passed to commands |
| `CODEX_HOME` | where the ChatGPT provider looks for `auth.json` (default `~/.codex`) |
