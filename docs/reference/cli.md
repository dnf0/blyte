# CLI flags

The output of `blyte --help` and of each subcommand's `--help`. Behaviour is described in [Running tasks](../usage.md), [the workflow](../orchestration.md), [Providers](../providers.md) and [Security](../security.md).

```text
A lean coding-agent harness with a sealed verification gate

Usage: blyte [OPTIONS] [PROMPT] [COMMAND]

Commands:
  dashboard  Serve a local dashboard of running and recent jobs
  dispatch   Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
  review     Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
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
  -h, --help
          Print help
  -V, --version
          Print version
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

## blyte dispatch

```text
Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary

Usage: blyte dispatch [OPTIONS] --gate <GATE> <ID> <TASK_FILE>

Arguments:
  <ID>         Identifier for this dispatch (used for worktree and branch wip/<id>)
  <TASK_FILE>  Path to the task description markdown file

Options:
  -g, --gate <GATE>                The check command; several lines are several steps, all of which must pass
      --tag <TEXT>                 Tag for this dispatch (repeatable)
      --base <BASE>                The commit a new worktree starts from [default: HEAD]
  -m, --model <MODEL>              Model identifier or provider selection
      --max-seconds <MAX_SECONDS>  Wall-clock budget in seconds [default: 3600]
      --max-turns <MAX_TURNS>      Maximum turn budget [default: 300]
      --max-cost <MAX_COST_USD>    Maximum spend in USD
      --fold-budget <FOLD_BUDGET>  Approximate token budget for the verbatim tail before folding
      --sandbox <SANDBOX>          Sandbox local command execution: 'on' or 'off' (default 'on') [default: on] [possible values: on, off]
      --network <NETWORK>          Outbound network mode: 'on' or 'off' (default 'on') [default: on] [possible values: on, off]
      --allow-read <ALLOW_READ>    Let sandboxed commands read this path despite credential deny-list (repeatable)
      --pass-env <PASS_ENV>        Pass this environment variable to commands (repeatable)
  -h, --help                       Print help
```

## blyte review

```text
Show what a dispatched run did: commits, files, tree state, report, and optionally the diff

Usage: blyte review [OPTIONS] <ID>

Arguments:
  <ID>  

Options:
      --diff  
  -h, --help  Print help
```

## blyte skill install

```text
A lean coding-agent harness with a sealed verification gate

Usage: blyte [OPTIONS] [PROMPT] [COMMAND]

Commands:
  dashboard  Serve a local dashboard of running and recent jobs
  dispatch   Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
  review     Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
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
  -h, --help
          Print help
  -V, --version
          Print version
```

## blyte skill uninstall

```text
A lean coding-agent harness with a sealed verification gate

Usage: blyte [OPTIONS] [PROMPT] [COMMAND]

Commands:
  dashboard  Serve a local dashboard of running and recent jobs
  dispatch   Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
  review     Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
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
`blyte.prev`, and refreshes the Claude Code skill if the installed copy is unedited.

## Environment

| variable | meaning |
|---|---|
| `BLYTE_MODEL` | default for `--model` |
| `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `OPENROUTER_API_KEY`, `OLLAMA_HOST`, `OLLAMA_API_KEY` | provider credentials and endpoints; never passed to commands |
| `CODEX_HOME` | where the ChatGPT provider looks for `auth.json` (default `~/.codex`) |
