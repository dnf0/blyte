These are facts about blyte's commands. They say what each command does, not how to run your work.

### Dispatching a task

The `blyte` tool runs blyte's own commands (`dispatch`, `wait`, `status`, `review`, `plan`, `init`,
`clean`) on the host in the repository's main checkout. Commands below are written as command lines;
the words after `blyte` are passed as `args`.

`blyte dispatch <id> <task.md>` starts one implementer on one task, in its own git worktree at
`.wt/<id>` on a branch `wip/<id>`. The implementer sees only the task text and the repository. A
dispatch takes minutes.

- `--plan <plan.md> --plan-task "Task <ID>"` takes the task from the plan section headed
  `### Task <ID>: …`, with no task file. Dispatch refuses a heading it cannot find. The implementer
  is given that section alone.
- `--base <branch>` starts the worktree from that branch, for example `--base wip/T1` for a task that
  needs T1's result.
- `--research` starts a read-only research run (see below).
- The gate is the repository's check, from `blyte.toml`. `--gate '<commands>'` overrides it; each
  line is a step and all must pass. If the repository has no `blyte.toml`, `blyte init` writes one
  from the repository's CI and prints a summary: a step **left out** failed for environment reasons
  and says why; a step that **failed on HEAD** fails on the current code. blyte refuses credentials
  in `blyte.toml`.
- Before the model starts, dispatch runs the gate on the untouched worktree. `--no-base-check` skips
  that, for a task meant to fix a check that already fails.
- The gate's first step (`blyte comments`) fails a run that adds comments other than documentation
  comments and tool directives.
- The model's API key comes from the environment or `~/.config/blyte/env`. A dispatch that has none
  fails with "no API key".

`blyte plan <plan.md>` lists a plan's tasks with their state (merged, review, running, dead, ready,
waiting) and prints, for each ready task, the exact `blyte dispatch` command.

### While runs are going

Each foreground `blyte` call and shell command times out after 300 s. Calling `blyte` with
`background: true` starts a `dispatch` and returns at once; nothing reports when it exits:

```json
{"args": ["dispatch", "T1", "--plan", "plan.md", "--plan-task", "Task T1"], "background": true}
{"args": ["dispatch", "T2", "--plan", "plan.md", "--plan-task", "Task T2"], "background": true}
{"args": ["wait", "--any", "T1", "T2"]}
```

- `blyte wait --any <ids>` prints `<id> <outcome> <turns> <cost>` for each run that has finished.
  Exit 2 means none finished within its timeout (280 s).
- `blyte status <id>` shows whether a run is still going, its turn, its last command and its gate
  attempts, or that it is `dead`.
- Each run's files: `.wt/<id>.json` (the summary), `.wt/<id>.log`, `.wt/<id>.task.md` (the task text
  it was given), and for research `.wt/<id>.report.md`.

The shell is sandboxed: it can write only in the repository and temporary directories, cannot read
credentials and has no network. A command that must leave the sandbox (for example `git push`) takes
`unsandboxed` with a reason and runs only when the user approves.

### What a run returns

One JSON object, also saved to `.wt/<id>.json`: `outcome`, `exit` (0 only for `verified`), `commits`,
`changed_files`, `final_message` (the implementer's own report), `gate_failures`, `turns`,
`cost_usd`, `seconds`, and the `log` and `report` paths.

| Outcome | Meaning |
|---|---|
| `verified` | The gate passed on the implementer's commits |
| `no_changes` | The gate passed on an untouched tree: nothing was done |
| `gate_failed` | The implementer stopped and the gate kept failing |
| `budget_exhausted` | The budget was spent without the gate passing |
| `blocked` | The gate already fails at the base commit; with zero turns, the environment or the base is broken, and the output names the step |
| `error` | The model provider or the environment broke; not a judgement of the work |

### Reviewing a run

`blyte review <id>` prints a summary. It starts with `== checks`: files the task does not name,
commit messages not in the task, a run that did not pass, added lines that look like a fallback,
added comments, host commands and reads outside the repository. `nothing flagged` means none of
these. `--diff` adds the full diff.

- `== host commands` lists logged commands that may have changed things outside the worktree.
- "the gate may have run in an environment from outside the worktree" means the implementer copied
  or linked a `.venv`, `node_modules` or cache entry from elsewhere, so the gate's pass does not
  cover the worktree alone.
- The gate runs the repository's checks. It cannot see external systems or real data.

`blyte review <id> --verdict accepted|fixed|redone --notes "<text>"` records what was done with a
run: `accepted` (used as it was), `fixed` (used after a change), `redone` (not used).

### Combining results

- An implementer never pushes or merges; its commits stay on `wip/<id>`.
- `git cherry-pick -x wip/<id>` records the source commit, and that line survives a squash merge, so
  blyte can tell the run was merged.
- A run's worktree and branch are removed by the next `blyte dispatch` or by `blyte clean` once its
  work is merged into the default branch or the branch it was dispatched from, once it is recorded
  as redone, or after 14 days.

### Research runs

`blyte dispatch <id> <question.md> --research` answers a question by reading. It works in a scratch
directory under `.wt/<id>`, reads the repository by absolute path, and cannot change it. It passes
when it has written a report whose cited lines and quoted code match the files, copied to
`.wt/<id>.report.md`. The check covers quotes, not conclusions. A report that starts with
`> **Not verified.**` came from a run that did not pass.

- A question has four headed sections, and dispatch warns about one without them (`--research` with
  no file prints the template): `## Objective`, `## Start from`, `## Out of scope`,
  `## Answer format`.
- The default budget is 300 turns and 30 minutes, with a warning ten turns before the end.
- Measured 2026-11-18: narrow questions (one subsystem, about ten files) finished in 3 to 10
  minutes; questions that made a run read a whole repository took the full 30 minutes and often
  failed. `broad research run` in `blyte review` marks such a run, and review lists the directories
  a run that ran out of budget read from.

### Measured limits of an implementer run

- Measured 2026-09-28 and 2026-10-02 (Gemini 3.8 Flash): a rename edited file by file across 72
  files used all 300 turns and failed.
- Measured in `docs/evidence.md`: splitting one feature into parallel dispatches cost 30–160% more
  and was slower than one dispatch; separate tasks dispatched at once finished about twice as fast
  overall as one after another.
