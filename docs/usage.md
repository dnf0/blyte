# Running tasks

## Headless runs

```bash
blyte "<task>" --gate "<check command>" [options]
```

blyte runs in the current directory, which should be a git repository: the gate's rollback and the
run report's baseline both use git. The task is plain text; say what to change and what "done"
looks like. A long task can be passed as a file's contents: `blyte "$(cat task.md)" --gate ...`.

## The gate

`--gate` (or `-g`) is the verification command. The model cannot see it as a tool: it cannot run
it, skip it or report on it. When the model says it is finished, blyte runs the gate:

- **exit 0**: the run is `verified` and blyte exits 0 (any other outcome exits 1).
- **non-zero**: the failure output goes back to the model and it tries again, up to
  `--max-gates` attempts (default 5).
- **several lines** are several steps. All steps run, and all failures are reported together.
- the gate has a 30-minute deadline per attempt; a model-issued command has 5 minutes.

With `--rollback` (the default), a failed attempt resets the working tree to the commit the run
started from (`git reset --hard` and `git clean -fd`) before the model tries again, so broken
attempts do not pile up and the model starts each attempt from known files. `--rollback=false`
keeps the tree as the model left it.

!!! warning "Start from a clean tree"
    Rollback discards uncommitted changes and untracked files, including any you made before the
    run. Commit or stash your own work first, or run blyte in its own worktree.

## Repository rules

blyte reads the repository's own rules for agents on every run and puts them in its system prompt,
however it was started (a direct `blyte dispatch`, a script, another agent). The first file found
wins: `.blyte/rules.md` (written for blyte), `.claude/agy-rules.md` (implementer rules kept for
headless agents), then `AGENTS.md`. `CLAUDE.md` is not read, because it is written for the
orchestrator. Each is looked for in the working tree, then in the main checkout (a rules file may
be untracked). Rules are capped at 12,000 characters. The run prints which file it used
(`blyte: repository rules from …`). Container runs read none.

## Dashboard

`blyte dashboard` serves a local web page that monitors running and recent blyte jobs across repositories:

```bash
blyte dashboard
```

On start, it prints `blyte dashboard: http://127.0.0.1:<port>` and serves until killed. The server binds to `127.0.0.1` only, and the page makes requests only to its own server—it runs strictly on this machine and makes no network calls. Pass `--open` to open the URL in your default browser automatically.

### Sources and configuration

By default, when neither `--root` nor `--runs-dir` is passed on the command line, `blyte dashboard` reads sources from `~/.config/blyte/dashboard.toml`:

```toml
roots = ["~/repos", "~/work"]
runs_dirs = ["~/.cache/blyte-runs"]
```

Leading `~/` paths expand to your home directory. If the config file is absent or lists no sources, `blyte dashboard` falls back to the current git repository's top-level checkout.

You can also pass sources explicitly:
- `--root <DIR>` (repeatable): directories containing repositories or `.wt` worktree folders.
- `--runs-dir <DIR>` (repeatable): directories containing `<repo>/<id>.log` run logs.
- `--hours <N>` (default 24): how many hours back to display finished or stopped jobs.
- `--port <N>` (default 7171): port to listen on; pass `0` to have the OS choose a free port.

## Budgets

| flag | default | stops the run when |
|---|---|---|
| `--max-turns N` | 25 | the model has taken N turns |
| `--max-gates N` | 5 | the gate has failed N times |
| `--max-seconds N` | none | N seconds of wall-clock time have passed; blyte stops itself and reports |
| `--max-cost USD` | none | list-price spend reaches USD (Gemini 3.8 Flash only for now; refused for a model with no known price) |

A run that hits a budget ends with outcome `budget_exhausted`.

## Context

`--context folded` (the default) keeps the prompt append-only so the provider's cache keeps
working, and folds the oldest turns into a digest once the verbatim part passes
`--fold-budget` tokens (default 48,000). `--context conversational` keeps everything, as a control.
For a local model with a small context, lower the budget: `--fold-budget 16000`.

## In a container

```bash
blyte "make the migration idempotent" --gate "pytest tests/test_migrations.py" \
  --container my-dev --container-workdir /app
```

With `--container <id>`, every command and file operation runs inside that running container, in
`--container-workdir` (default `/workspace`), through one persistent `docker exec` session.
Container commands never inherit the host's environment. The host sandbox does not apply: isolate
the container itself (its network, mounts and user).

## Reports

`--report path.json` writes a JSON record when the run exits, whatever the outcome: outcome, turns,
cost, per-call token usage, where the time went, gate failures and the security settings. See
[Run reports](reports.md).

## Driving blyte from another agent

blyte is built to be dispatched: one task per run, in its own worktree, with a gate and a report,
reviewed and merged by whoever dispatched it. [blyte as your implementer](orchestration.md)
describes the pattern, `blyte dispatch` and `blyte review`, and the Claude Code skill.
