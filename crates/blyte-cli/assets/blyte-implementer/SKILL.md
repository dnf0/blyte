---
name: blyte-implementer
description: Use by default whenever you would otherwise write the code yourself for a change in a git repository that has a check command (tests, lint): plan the change, split it into independent tasks and dispatch them to blyte in parallel (`blyte dispatch`, one worktree each, on a cheaper model, with the check as the gate), then check each result and merge. Skip it for design discussions, ambiguous requirements, one-line edits, and anything with no check.
---

# blyte as your implementer

You plan; blyte implements, on a cheaper model, and its gate (the repository's check command)
decides whether the work is done. **The plan is where your effort pays most.** Measured on a real
task (2026-09-28): given a frontier model's plan, blyte passed every time at a fraction of the cost,
with or without a detailed review; from its own plan it failed. So make each task unambiguous and
its tests decisive, run independent tasks at the same time, and let a passing gate do most of the
checking.

## When to delegate

Delegate when a careful engineer could do the task from its description alone, without asking
anything, and the repository has a check that would catch a wrong result. Keep for yourself:
design decisions, ambiguous requirements, one-line edits, anything with no check, and anything
touching credentials, deployment or other repositories.

## 1. Plan for parallel work (the default)

Parallel is the default: it is how this workflow saves time. Break the change into tasks of
one commit each, arranged so that as many as possible touch disjoint files, then sort them:

- **Independent tasks** touch disjoint files and do not depend on each other: they run in parallel.
- **Dependent tasks** need an earlier task's result: they run after it, based on its branch
  (`--base wip/T1`).
- **Coupled tasks** (task 2 cannot pass its checks without task 1) are one task.

Give every task a **Depends on:** line: `none`, or the tasks whose result it needs. Tasks that
change the same file depend on each other. When parallel tasks would each register something in one
shared file (a registry, `definitions.py`, a route table), have each create its own module and leave
the shared-file wiring to a small follow-up task or to you at merge; otherwise they conflict. Then:

- Dispatch every independent task at once. That means every task whose dependencies are merged.
- When a merge unblocks a task, dispatch it straight away, before you review the next result, so
  blyte is never idle while you read.
- Run tasks one after another only when one needs another's result. If a plan has two or more
  tasks with `Depends on: none` and you are about to run them in turn, stop: that is a mistake.

A plan with a single task is fine for a single change; do not split work artificially just to run
it in parallel.

## 2. Write each task

One task per dispatch. When the task is a `### Task <ID>` section of a plan file, write it there and
dispatch with `--plan <plan.md> --plan-task "Task <ID>"` and no task file: blyte takes that section
alone, so it does not start on the other tasks. Otherwise write the task to its own Markdown file.
Either way blyte sees only the task and the repository, nothing from your conversation (it keeps the
text it was given at `.wt/<id>.task.md`). This is the step to spend your effort on: the gate can only check what the task's
tests pin down. Include:

- **Goal**, in one or two sentences, and **why**.
- **Files** to create or change.
- **What to build**: names, signatures and behaviour that are part of the contract, including
  every edge case the change has to handle; exact code where it matters.
- **Tests** that prove it, written out, one for each behaviour and edge case, and the command that
  runs them. If a wrong implementation could pass them, the task is not finished.
- **How**, when it matters: for a bulk mechanical change (a rename across many files), say to do it
  with one script (`git mv`, `sed`) and then fix the exceptions by hand; otherwise the implementer
  may edit file by file and run out of turns. A task that would touch more than about 20 files must
  either say which script makes the change or be split into several tasks: edited one by one, a
  rename across 72 files and 72 logger call sites each used all 300 turns and failed (2026-09-28,
  2026-10-02).
- **Constraints**, e.g. "existing tests pass unmodified".
- **Depends on**: `none`, or the tasks it needs, as in the plan.
- **Siblings**: for a task running in parallel, name the other tasks and say they are **not** in
  this worktree ("T2 adds `slugify` separately; do not add it here"), so it does not "fix" their
  absence.
- **The exact commit message.**

Before dispatching, check that the tests fail on the current code and can pass with the intended
change.

## 3. Dispatch

From the repository's main checkout. Each dispatch takes minutes.

```bash
blyte dispatch T1 /abs/path/task-T1.md --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

- **The plan:** when the task comes from a plan file, say which one:
  `--plan docs/plans/my-plan.md --plan-task "Task 2: the parser"` (the task's heading). The
  dashboard's Plans view then shows the plan as a flow of its tasks, each coloured by its state.
  A follow-up task for work that has a plan (a fix found in review, an extra step) takes the same
  `--plan` with its own `--plan-task` name, such as `--plan-task "H2: pin signature is data only"`;
  do not edit the plan file for it. The Plans view shows it as added during the work.

  `blyte plan <plan.md>` lists the plan's tasks, what has run, and the exact dispatch command for each
  task that is ready: start there instead of working it out from the plan and `.wt/`.

- **The gate:** if the repository has a `blyte.toml`, leave `--gate` out: blyte uses the
  repository's own checks. If it has none, run `blyte init` first. It takes the checks from the
  repository's CI, sets what the environment needs (`[dispatch.env]`, such as `UV_FROZEN`), runs
  each step once in a fresh worktree, and writes `blyte.toml` with a comment on every step. Read
  that summary: a step **left out** failed for environment reasons (credentials, network) and
  says why; a step that **failed on HEAD** is a real failure to fix, or to tell the user about,
  before dispatching. Each line of the gate is a step; all must pass; the slow full test run goes
  last. Never put credentials in `blyte.toml`; blyte refuses them.
- The implementer model's API key must be in the environment or in `~/.config/blyte/env` (`NAME=value`, `chmod 600`); blyte never prints it. If a dispatch fails with "no API key", tell the user; do not do the work some other way.

**Starting parallel tasks.** If your harness tells you when a background command exits (an
interactive Claude Code session does), send every independent `blyte dispatch` as its own
background command in the same message and review each as it lands. Otherwise (Codex, or a
one-shot session such as `claude -p`, where background commands die with the session), start
them together and wait in the foreground:

```bash
blyte dispatch T1 /abs/task-T1.md > /dev/null &
blyte dispatch T2 /abs/task-T2.md > /dev/null &
wait
```

**Under `blyte --orchestrate`** (your commands run in a shell that keeps background jobs alive, but you
are not told when they exit, and each command times out after 300 s): start each dispatch in the
background with both outputs redirected, or the command waits for it, then wait with `blyte wait`:

```bash
blyte dispatch T1 --plan plan.md --plan-task "Task T1" > /dev/null 2>&1 &
blyte dispatch T2 --plan plan.md --plan-task "Task T2" > /dev/null 2>&1 &
blyte wait --any T1 T2
```

`blyte wait` prints `<id> <outcome> <turns> <cost>` for each run that has finished. Review those, drop them
from the list and call it again for the rest. Exit 2 means none finished within its timeout (280 s): call
it again.

Each dispatch also saves its summary to `.wt/<id>.json`, so nothing is lost when output is
discarded. `blyte dashboard` shows every dispatch on the machine while they run.

**A long dispatch:** `blyte status T1` shows whether it is still running, the current turn, the
last command and gate attempts. A run that is `dead`, or many turns in with no gate attempt,
is worth a look at its log (`.wt/T1.log`) before you wait longer.

**Under Codex:** run `blyte dispatch` and `blyte review` outside Codex's sandbox, with escalated
permissions: set `sandbox_permissions` to `"require_escalated"` on that shell command, with a
one-line justification. They reach blyte's model over the network and create git worktrees, which
Codex's sandbox does not allow (it makes `.git` read-only); blyte applies its own sandbox to
everything the implementer runs. `blyte skill install --harness codex` pre-approves exactly these
two commands, so no prompt appears. Merging a result into the main checkout also writes to `.git`:
run it with escalated permissions too, or leave the merge to the user. If `blyte dispatch` cannot
run, stop and say why; do not implement the change yourself.

Each dispatch prints one JSON object: `outcome` (`verified`, `no_changes`, `gate_failed`,
`budget_exhausted`, `error`), `exit` (0 only for `verified`), `commits`, `changed_files`,
`final_message` (blyte's own report), `gate_failures`, `turns`, `cost_usd`, `seconds`, and the
`log` and `report` paths. `no_changes` means the gate passed on an untouched tree: not done.

## 4. Check each result: trust the gate, not the summary

```bash
blyte review T1
```

It starts with `== checks`: files the task does not name, commit messages not in the task, a run that
did not pass, host commands and reads outside the repository. Add `--diff` for the full diff when
something is flagged or surprising; otherwise the summary is usually enough, and it keeps your context small.
If blyte review prints "blyte: <version> is out", tell the user, with that line.

**When the gate passed (`verified`)**, a light check is enough:

1. **Scope**: only the files the task names changed, in the commit(s) you asked for, with the exact message
   (`== checks` says "nothing flagged" when this holds).
2. **Deviations**: every deviation in `final_message` is justified, and the diff shows none it did not mention.
3. **Out-of-scope effects**: `blyte review` lists, under `== host commands`, logged commands that
   may have changed things outside the worktree (deleted a cache, wrote to `~/`, pushed). If any
   appear, check what they did and tell the user.

You do not need to re-run the gate: it ran the repository's checks and they passed.

The gate cannot see the outside world. When the change reads or writes external systems (cloud
storage, APIs, regions, real data volumes), run a read-only check against real data before merging:
a passing test suite has missed a client defaulting to the wrong region, a listing that would take
hours on a real prefix, and a missed input folder.

**Review in depth** (read the whole diff, re-run the gate in `.wt/T1`, check verbatim code landed
verbatim) when the gate failed or the budget ran out, when the change is risky (security, data
handling, migrations, a public interface), or when the diff surprises you.

- `blocked` means the check already fails at the base commit, so the task cannot pass until that is fixed: confirm it, then fix the base (or tell the user) before redispatching, rather than redispatching as it is.

**Record your verdict** once you have checked it, so the repository keeps an honest record of
how blyte does:

```bash
blyte review T1 --verdict accepted --notes "to the task"      # merged as it was
blyte review T1 --verdict fixed --notes "what you changed"    # merged after a small fix
blyte review T1 --verdict redone --notes "why"                # not used; redone or rewritten
```

A run's worktree is removed once its work is merged into the default branch or into the branch it was dispatched from (or it was redone, or 14 days have passed): by the next `blyte dispatch` in the repository or by `blyte clean`. Until then you can still test or fix in it.

If it falls short: fix a trivial issue yourself, or write a follow-up task saying exactly what to
change and dispatch it (`blyte dispatch T1b follow-up.md --base wip/T1 --gate ...`). If the task was
wrong, fix the task, not the result.

## 5. Merge

Merge verified tasks one at a time:

```bash
git merge --ff-only wip/T1
```

When an earlier merge has moved the branch, a parallel task no longer fast-forwards: rebase it
first (`git -C .wt/T2 rebase <branch>`), re-run its gate in the worktree, then merge it. To combine
several runs on a new branch instead, use `git cherry-pick -x wip/T1 wip/T2`: the `-x` line survives
a squash merge, so blyte can tell the runs were merged. After the last merge, run the full check once
more on the merged branch. Push or open a pull request only if the user asks. Leave the worktrees and
`wip/` branches: blyte removes them once their work is merged.

## Read-only research

A question that would otherwise take many file reads of your own (how something works, where
something is used, what a change would break) can go to blyte too, so the reading costs the cheaper
model. Write the question to a file, saying what the answer must contain (paths with line numbers, a
table, a recommendation), and from the repository:

```bash
blyte dispatch R1 /abs/question-R1.md --research
```

blyte works in a scratch directory under `.wt/R1`, reads the repository by absolute path, and cannot
change it; the run passes when it has written a report whose cited lines and quoted code match the
files, copied to `.wt/R1.report.md` (and to `--report-to <path>`). Leave the budget at its default (300 turns, 30 minutes) rather than tightening it: a run cut short wastes what it spent, and it is warned ten turns before the end to finish its report. The check covers quotes, not conclusions: check any claim you will act on against the code it cites.

## Rules

- One task per dispatch; never hand over a whole plan.
- blyte never pushes, merges or reads secrets. If its log shows otherwise, stop and tell the user.
- Report to the user what **you verified**, not what blyte claimed.
