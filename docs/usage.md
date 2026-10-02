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

Gates take a machine-wide slot (`BLYTE_GATE_SLOTS`, defaulting to a quarter of the cores, at least 2)
so parallel runs do not saturate the machine running full test suites at once. Time spent waiting
for a slot does not count against `--max-seconds`. Research runs (`blyte dispatch --research`) and
runs with `--no-gate-slot` skip slot acquisition.

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

## Repository configuration (`blyte.toml`)

`blyte init` works out a repository's gate and environment settings, and proves each step in a
fresh worktree before writing `blyte.toml`. It first extracts check commands from CI workflows
(`.github/workflows/*.yml` and `*.yaml`), stripping reporting arguments (like `--cov` and `| tee`),
or falls back to toolchain detection (Makefiles, Cargo, pyproject, package.json, go.mod). Of overlapping
pytest steps that test the same files, it keeps only the broader one. It detects
environment needs such as `UV_FROZEN = "1"` (when `uv.lock` exists) and `UV_OFFLINE = "1"` (when a
private index is configured). Unless `--no-verify` is passed, it proves each step in a detached
worktree of `HEAD` (`.wt/init-<pid>`) using the sandboxed local shell, leaving out steps broken by
the environment and flagging steps that fail on HEAD. When every step taken from CI fails on HEAD
or is left out, it verifies the checks detected from the repository and uses them if any passes.
`blyte dispatch` reads `blyte.toml` when flags are not passed. In a linked worktree with no
`blyte.toml` of its own, it falls back to the main checkout's:

```toml
# blyte.toml: how this repository's work is checked, for `blyte dispatch`.
[dispatch]
gate = """
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --all-targets
"""
# model = "gemini-3.8-flash-high"
max_seconds = 3600
max_turns = 300
# max_cost = 2.0
# network = "on"
# fold_budget = 60000

# [dispatch.env]
# UV_FROZEN = "1"   # settings for every command and the gate; never credentials
```

| key | type | meaning |
|---|---|---|
| `gate` | string | the check command; several lines are several steps |
| `model` | string | model identifier or provider selection |
| `max_seconds` | integer | wall-clock budget in seconds (default 3600) |
| `max_turns` | integer | maximum turn budget (default 300) |
| `max_cost` | float | maximum spend in USD |
| `fold_budget` | integer | approximate token budget for verbatim tail before folding |
| `network` | string | outbound network mode (`"on"` or `"off"`, default `"on"`) |
| `[dispatch.env]` | table | environment variables set for every command and the gate; never credentials |

When `blyte dispatch` uses the gate from `blyte.toml` (not an explicit `--gate` or `--research`),
it checks whether any check commands run by the repository's CI workflows (`.github/workflows/*.yml`
and `*.yaml`) are missing from the gate. If any CI step is not covered (either in the gate or documented
in `blyte.toml` comments as deliberately left out), it warns on stderr before the run starts:

```
blyte: the gate is missing CI steps: make typecheck, make validate (run blyte init --force to refresh blyte.toml)
```

and continues the dispatch normally without modifying `blyte.toml`.

## Dashboard

`blyte dashboard` serves a local web page that monitors running and recent blyte jobs across repositories:

```bash
blyte dashboard
```

On start, it prints `blyte dashboard: http://127.0.0.1:<port>` and serves until killed. The server binds to `127.0.0.1` only, and the page makes requests only to its own server—it runs strictly on this machine and makes no network calls. The page uses blyte's bundled Archivo and IBM Plex Mono fonts, under the SIL Open Font License, served locally. Pass `--open` to open the URL in your default browser automatically.

The page refreshes every 5 seconds. Running jobs come first, then jobs that finished in the last
24 hours. Each row shows the job's status, title, what it is doing now (the last tool call, "Gate
running" while its check runs, or the first line of its final message), repository, time, cost
and turns. Click a row for the task (its own section of the plan, when the prompt names a plan and
a task title), the last tool, gate failures, changed files, commits, worktree, log and verdict.
The summary strip counts running jobs, jobs and repositories in the last 24 hours, the share that
passed their check, and their cost. Selecting text in a row pauses the list's refresh until you
clear the selection, so a path can be copied.

### Statuses

| Status | Meaning |
|---|---|
| Running | The job's process is alive (for a run directory, only if that process started with the job, so a reused PID does not count) |
| Verified | The run's report says its gate passed |
| Gate failed · Out of budget · No changes | The report's outcome |
| Error | The run stopped with an error (a blyte report, or an agy summary with `"status": "ERROR"`) |
| Finished | An agy run that ended without error; agy runs no gate of its own |
| Stopped | No report, and the process is gone |

### Tags

Every job is tagged `repo: <name>`, and `plan: <file stem>` when its prompt names a plan. Click a
tag to show only the jobs that carry it; click it again, or **Clear**, to show all. More tags come
from `blyte dispatch --tag "<text>"` (repeatable), or from a `<id>.tags.json` file (a JSON array of
strings) beside a job's log. A `<id>.verdict.json` file (`{"verdict": "accepted", "notes": "..."}`)
adds the reviewer's verdict to the details.

### What it can see

The dashboard reads files; it does not watch processes. It shows:

- **`blyte dispatch` jobs**: `<root>/.wt/<id>.start.json` and `<root>/<repo>/.wt/<id>.start.json`
  (one level below each root), with the log, report and JSON summary beside it.
- **Run directories**: `<runs-dir>/<repo>/<id>.log`, with an optional `<id>.pid`, `<id>.prompt`,
  `<id>.report.json` (blyte's `--report`), `<id>.json` (an agy summary) and `<id>.worktree`.
  Scripts that start blyte or agy in the background can write these.

A plain `blyte "..."` run in a terminal writes none of these, so it does not appear; start it with
`blyte dispatch`, or write its log and `--report` into a run directory.

### Sources and configuration

By default, when neither `--root` nor `--runs-dir` is passed on the command line, `blyte dashboard` watches every repository where `blyte dispatch` has been run on this machine, plus any extra sources configured in `~/.config/blyte/dashboard.toml`:

```toml
roots = ["~/repos", "~/work"]
runs_dirs = ["~/.cache/blyte-runs"]
```

Leading `~/` paths expand to your home directory. Every dispatch records its repository in `$BLYTE_STATE_DIR/repos` (default `$XDG_STATE_HOME/blyte/repos` or `~/.local/state/blyte/repos`), so the dashboard finds all of them from any folder without needing to be started in the repository.

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
| `--max-seconds N` | none | N seconds of wall-clock time have passed; blyte stops itself and reports. The run stops, with a report, at most 30 s after the budget even if a model call or command never returns |
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
`--plan <path>` and `--plan-task <text>` record which plan task the dispatch implements, so the
dashboard can show plans.

### Recording a verdict

Record your verdict after reviewing a run with `blyte review <id> --verdict <accepted|fixed|redone> [--notes <text>]`. This writes `<root>/.wt/<id>.verdict.json`, which `blyte dashboard` displays beside the run. Recording the verdict removes the worktree only when the clean rules allow it (in practice: `redone`, or the work is already merged); otherwise it prints `worktree .wt/<id> kept until wip/<id> is merged` (for `accepted`/`fixed`). A worktree with uncommitted changes is kept. It works even after the worktree has been removed. Without `--verdict`, `blyte review` flags commands that may have changed things outside the worktree under `== host commands`, and reads of other terminals or files outside the repository under `== reads outside the repository`.

## Cleaning finished worktrees (`blyte clean`)

Finished dispatches leave their worktree at `.wt/<id>` and branch `wip/<id>`. Worktrees are cleaned up automatically on review and at the start of new dispatches, or on demand with `blyte clean`:

```bash
blyte clean [--dry-run] [--older-than DAYS] [--all]
```

`blyte clean` inspects `.wt/<id>` directories with a corresponding `.wt/<id>.start.json`. A worktree is removable when the run is finished (no live process, or a `.wt/<id>.json` summary exists) and one of:
1. its work is on the default branch, or on the branch the run was dispatched from (`--base`) (`merged`, or `merged into <base>`);
2. it was discarded: the verdict is `redone` (`verdict redone`), or the outcome in `.wt/<id>.json` is `no_changes` (`no changes`);
3. it is a research run (`research: true`) and its report was copied out (`.wt/<id>.report.md` exists);
4. it finished more than `--older-than` days ago (default 14), by the summary file's mtime (`finished over N days ago`).

A verdict of `accepted` or `fixed` on its own no longer makes a worktree removable; the kept reason for such a run is `not merged yet`. A worktree with uncommitted changes (`git status --porcelain` not empty) is never removed, and is reported as kept (`kept .wt/<id> (uncommitted changes)`). Research scratch directories are removed directly; git worktrees are removed with `git worktree remove` (without `--force`) and `git worktree prune`. The run's record files (`.wt/<id>.log`, `.report.json`, `.json`, `.start.json`, `.verdict.json`, `.report.md`) are never removed.

The branch's work is on the default branch, or on the branch the run was dispatched from (`--base`), when any of:
- `git merge-base --is-ancestor <tip> <default>` succeeds;
- every commit `git cherry <default> <branch>` lists is marked `-`;
- merging it would change nothing: `git merge-tree --write-tree <default> <branch>` exits 0 and the tree id it prints equals `git rev-parse <default>^{tree}`.

The default branch is `origin/HEAD`'s target, else `main`, else `master`. Branches are deleted only when merged; `--older-than` never deletes a branch.

- `--dry-run`: shows what would be removed and deleted without making any changes.
- `--older-than DAYS`: worktrees older than this many days are removable without a verdict (default 14).
- `--all`: cleans every repository in the registry (`~/.local/state/blyte/repos`).

`blyte clean` also reports other worktrees (such as sessions' sibling worktrees) whose work is merged and working tree is clean, listing the `git worktree remove` commands to delete them without removing them itself.

Before creating a new worktree, `blyte dispatch` (except for `--research`) quietly cleans the repository, printing `blyte: removed N finished worktrees (X GB)` on stderr only if worktrees were removed.

## Research

Read-only investigations ("how does X work", "what would break if Y") save model usage when blyte
does the reading and writes a report. Run them as a dispatch with `--research`:

```bash
blyte dispatch Q1 question.md --research [--report-to <path>]
```

In research mode, blyte runs in an isolated scratch repository under `.wt/<id>` instead of creating
a worktree of the repository. The dispatch passes `--read-root <repository root>` to the child run,
allowing the `read` and `grep` tools to inspect the repository by absolute path while writing remains
strictly sealed to the scratch workspace. The task prompt directs the model to read the code under the repository root
by absolute path and write findings to `report.md`. The gate is `test -s report.md`, verifying that a
non-empty report was written. When finished, the report is copied to `.wt/<id>.report.md` (and to
`--report-to` when given), and `blyte review <id>` displays its contents. Research runs default to
300 turns and 30 minutes (ignoring `blyte.toml`'s budget, though explicit `--max-turns` and
`--max-seconds` flags still win).
