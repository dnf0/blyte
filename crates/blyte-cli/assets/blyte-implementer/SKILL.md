---
name: blyte-implementer
description: Use by default whenever you would otherwise write the code yourself for a change in a git repository that has a check command (tests, lint): plan the change, split it into precise tasks, dispatch each to blyte (`blyte dispatch`, in its own worktree, on a cheaper model, with the check as its gate), independent tasks in parallel, then verify every result yourself and merge. Skip it for design discussions, ambiguous requirements, one-line edits, and anything with no check.
---

# blyte as your implementer

You plan and review; blyte implements, on a cheaper model, and its gate (the repository's check
command) decides whether the work is done. Your job is to make each task unambiguous, run
independent tasks at the same time, and verify every result, not to trust it.

## When to delegate

Delegate when a careful engineer could do the task from its description alone, without asking
anything, and the repository has a check that would catch a wrong result. Keep for yourself:
design decisions, ambiguous requirements, one-line edits, anything with no check, and anything
touching credentials, deployment or other repositories.

## 1. Plan, and split for parallel work

Break the change into tasks of one commit each. Then sort them:

- **Independent tasks** touch disjoint files and do not depend on each other: they run in parallel.
- **Dependent tasks** need an earlier task's result: they run after it, based on its branch
  (`--base wip/T1`).
- **Coupled tasks** (task 2 cannot pass its checks without task 1) are one task.

## 2. Write each task

One task per Markdown file. blyte sees only this file and the repository, nothing from your
conversation. Include:

- **Goal**, in one or two sentences, and **why**.
- **Files** to create or change.
- **What to build**: names, signatures and behaviour that are part of the contract; exact code
  where it matters.
- **Tests** that prove it, written out, and the command that runs them.
- **Constraints**, e.g. "existing tests pass unmodified".
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

- Each line of the gate is a step; all must pass. Put the slow full test run last.
- The implementer model's API key must be in the environment; blyte never prints it.

**Parallel tasks in an interactive session:** send every independent `blyte dispatch` as its own
background command in the same message. You are notified as each one exits; review each as it
lands instead of waiting for all of them.

**Parallel tasks in a one-shot or headless session** (for example `claude -p`): background commands
die when the session ends, so start them together and wait in the foreground:

```bash
mkdir -p .wt
blyte dispatch T1 /abs/task-T1.md --gate '...' > .wt/T1.json &
blyte dispatch T2 /abs/task-T2.md --gate '...' > .wt/T2.json &
wait
```

Each dispatch prints one JSON object: `outcome` (`verified`, `no_changes`, `gate_failed`,
`budget_exhausted`, `error`), `exit` (0 only for `verified`), `commits`, `changed_files`,
`final_message` (blyte's own report), `gate_failures`, `turns`, `cost_usd`, `seconds`, and the
`log` and `report` paths. `no_changes` means the gate passed on an untouched tree: not done.

## 4. Verify each result. Do not trust the summary

```bash
blyte review T1 --diff
```

1. **Scope**: only the files the task names changed, in the commit(s) you asked for, with the exact message.
2. **Fidelity**: code you gave verbatim landed verbatim; read everything else in full.
3. **Checks yourself**: run the gate in the worktree (`.wt/T1`). A passing gate is necessary, not sufficient.
4. **Deviations**: every deviation in `final_message` is justified, and there are none it did not mention.
5. **Out-of-scope effects**: skim the log (`.wt/T1.log`) for commands that touched anything outside the worktree.

If it falls short: fix a trivial issue yourself, or write a follow-up task saying exactly what to
change and dispatch it (`blyte dispatch T1b follow-up.md --base wip/T1 --gate ...`). If the task was
wrong, fix the task, not the result.

## 5. Merge

Merge verified tasks one at a time:

```bash
git merge --ff-only wip/T1
git worktree remove .wt/T1 && git branch -d wip/T1
```

When an earlier merge has moved the branch, a parallel task no longer fast-forwards: rebase it
first (`git -C .wt/T2 rebase <branch>`), re-run its gate in the worktree, then merge it. After the
last merge, run the full check once more on the merged branch. Push or open a pull request only if
the user asks.

## Rules

- One task per dispatch; never hand over a whole plan.
- blyte never pushes, merges or reads secrets. If its log shows otherwise, stop and tell the user.
- Report to the user what **you verified**, not what blyte claimed.
