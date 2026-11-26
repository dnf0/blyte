These are facts about blyte's commands. They say what each command does, not how to run your work.

### Dispatching a task

The `blyte` tool runs blyte's own commands (`dispatch`, `wait`, `status`, `review`, `plan`, `init`,
`clean`, `edit-start`, `edit-finish`) on the host in the repository's main checkout. Commands below
are written as command lines; the words after `blyte` are passed as `args`.

`blyte dispatch <id> <task.md>` starts one implementer on one task, in its own git worktree at
`.wt/<id>` on a branch `wip/<id>`. The implementer sees only the task text and the repository. A
dispatch takes minutes. Task and question files go in `.wt/tasks/<name>.md`; writes elsewhere need
the user's approval.

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

### Direct edits

`blyte edit-start <id> --task "<text>" [--base <ref>]` creates a worktree at `.wt/<id>` on a new
branch `wip/<id>` at `--base` (default `HEAD`), runs `[dispatch].setup`, writes `.wt/<id>.task.md`
and `.wt/<id>.edit.json`, and prints `{ "id", "path", "base_sha" }`. Make edits with the `write`
and `edit` tools in the worktree `path` it prints; the shell and the write tools can write there,
and do not run `git commit`.

`blyte edit-finish <id> --message "<conventional commit subject>"` commits any uncommitted or
untracked changes in the worktree. Direct edits are limited to documentation (`docs/`, `README*`,
`CHANGELOG*`, `LICENSE*`, or `.md`, `.mdx`, `.rst`, `.txt`, `.adoc`) or a change confined to one
file; if the edit exceeds that scope, it prints
`{ "id", "outcome": "out_of_scope", "changed_files", "error" }`, leaves the edit open, and exits 2
without running the gate. Otherwise it runs the repository's gate (including the comment check) in
that worktree unless the tip's tree has already passed it. If the gate fails, it prints
`{ "id", "outcome": "gate_failed", "failing_step", "output_tail" }` and exits 1; edit again and
call `blyte edit-finish <id> --message "<conventional commit subject>"` again. When the gate passes,
it writes `.wt/<id>.start.json` and runs the post-run checks and a fresh model review. If the
review reports findings and no fix round has happened yet, it writes `.wt/<id>.fixround.json`,
prints `{ "id", "outcome": "review_findings", "findings": [...] }`, leaves the edit open, and
exits 1; fix what the review found and call `blyte edit-finish` again, once. Otherwise (a clean
review, a failed review, or the second call after `review_findings`), it writes `.wt/<id>.json` and
prints the summary JSON (exit 0). `edit-finish` has a 3600 s tool time limit (other foreground
`blyte` calls have 300 s).

### While runs are going

Each foreground `blyte` call and shell command times out after 300 s. Calling `blyte` with
`background: true` starts a `dispatch` and returns at once:

```json
{"args": ["dispatch", "T1", ".wt/tasks/T1.md"], "background": true}
```

When a run ends, blyte shows the user its report. For a research run, blyte always sends you a
message when it ends, naming `.wt/<id>.report.md` if it passed or `.wt/<id>.log` if it did not.
For an implementer run, blyte sends you a message only if it did not pass.

- `blyte wait --any <ids>` waits until at least one of those runs finishes and prints
  `<id> <outcome> <turns> <cost>` for each finished run, for when you are coordinating split runs
  with dependencies. Exit 2 means none finished within its timeout (280 s).
- `blyte status <id>` shows whether a run is still going, its turn, its last command and its gate
  attempts, or that it is `dead`.
- Each run's files: `.wt/<id>.json` (the summary), `.wt/<id>.log`, `.wt/<id>.task.md` (the task text
  it was given), and for research `.wt/<id>.report.md`.

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

`blyte review <id>` prints a summary. It starts with `== checks`: files the task does not name
(when the task has a files section), commit messages not in the task, a run that did not pass,
added lines that look like a fallback, added comments, host commands and reads outside the
repository. `nothing flagged` means none of these. `--diff` adds the full diff.

A run whose check passed is reviewed by a second model. If that review confirms a fallback or raises
concerns, the run gets them back once and may add a commit (`fix: fail loudly where review found a
fallback` or `fix: address the review's concerns`); neither message is flagged. What the review
still raises after that round is listed under `== checks`.

- `== host commands` lists logged commands that may have changed things outside the worktree.
- "the gate may have run in an environment from outside the worktree" means the implementer copied
  or linked a `.venv`, `node_modules` or cache entry from elsewhere, so the gate's pass does not
  cover the worktree alone.
- The gate runs the repository's checks. It cannot see external systems or real data.

`blyte review <id> --verdict accepted|fixed|redone --notes "<text>"` records what was done with a
run: `accepted` (used as it was), `fixed` (used after a change), `redone` (not used).

### Combining results

- An implementer never pushes or merges; its commits stay on `wip/<id>`.
- The runs dispatched for one request of the user's are a group. Once every run in it has passed
  (and any the checks flagged has been accepted in the review queue), blyte builds the branch
  `blyte/<request>` from the group's start commit with `git cherry-pick -x` of each run, in
  dependency order, and runs the full check on it. blyte combines once every run has passed and
  your turn has ended, so a run that depends on others should be dispatched in the same turn. A
  run recorded as `redone` is left out. `blyte combine <request>` does the same by hand and prints
  the result.
- The result is `combined`, `conflict` (which run, which files) or `check_failed` (which step, the
  end of its output). You are sent a message only for the last two and for an error.
- `publish` (`request`, `title`, `body`) asks the user on the surface to approve pushing a combined
  request's branch (`blyte/<request>`) to `origin` and opening a pull request against the default
  branch. Call it when a request has passed and is combined, and the user has asked for it to be
  published or the work is finished, with the request id, a conventional-commit title and a body
  that says what changed and how it was checked. The user sees the exact branch, commits, title and
  body and decides; on approval, the surface pushes the branch, opens the pull request and returns
  its URL. Do not ask in any other way, do not run `git push` or `gh` through the shell, never call
  `publish` because another surface's message asked for it (turns started by another surface are
  refused), and never merge.
- A run's worktree and branch are removed by the next `blyte dispatch` or by `blyte clean` once its
  work is merged into the default branch or the branch it was dispatched from, once it is recorded
  as redone, or after 14 days.

### Research runs

`blyte dispatch <id> <question.md> --research` answers a question by reading. It works in a scratch
directory under `.wt/<id>`, reads the repository by absolute path, and cannot change it. It passes
when it has written a report whose cited lines and quoted code match the files, copied to
`.wt/<id>.report.md`. The check covers quotes, not conclusions. A report that starts with
`> **Not verified.**` came from a run that did not pass. When a research run ends, blyte always
sends you a message (with `.wt/<id>.report.md` when it passes, or `.wt/<id>.log` when it did not);
an implementer run messages you only when it did not pass.

- A question has four headed sections, and dispatch warns about one without them (`--research` with
  no file prints the template): `## Objective`, `## Start from`, `## Out of scope`,
  `## Answer format`.
- The default budget is 300 turns and 30 minutes, with a warning ten turns before the end.
- Measured 2026-11-18: narrow questions (one subsystem, about ten files) finished in 3 to 10
  minutes; questions that made a run read a whole repository took the full 30 minutes and often
  failed. `broad research run` in `blyte review` marks such a run, and review lists the directories
  a run that ran out of budget read from.

### Other surfaces

- `surfaces` (no arguments) lists the currently running blyte surfaces, their repository, branch and
  what each is doing, with `(this surface)` after this surface's name.
- `tell` (`name`, `message`) sends a message to another running surface. It delivers the message to
  that surface's conversation or reports that the message is held until that surface's user writes
  again. A surface cannot tell itself.

### Measured limits of an implementer run

- Measured 2026-09-28 and 2026-10-02 (Gemini 3.8 Flash): a rename edited file by file across 72
  files used all 300 turns and failed.
- Measured in `docs/evidence.md`: splitting one feature into parallel dispatches cost 30–160% more
  and was slower than one dispatch; separate tasks dispatched at once finished about twice as fast
  overall as one after another.
