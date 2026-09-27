---
name: blyte-implementer
description: Use when a task can be handed to a cheaper model to implement while you plan and review: write a precise task, dispatch it with `blyte dispatch` into its own git worktree with the repository's check command as its gate, then verify the result yourself and merge it. Use for well-specified changes in a git repository with a runnable check (tests, lint).
---

# blyte as your implementer

You plan and review; blyte implements, on a cheaper model, and its gate (the repository's check
command) decides whether the work is done. Your job is to make each task unambiguous and to verify
the result, not to trust it.

## When to delegate

Delegate when a careful engineer could do the task from its description alone, without asking
anything, and the repository has a check that would catch a wrong result. Keep for yourself:
design decisions, ambiguous requirements, anything with no check, and anything touching
credentials, deployment or other repositories.

## 1. Write the task

One task per dispatch, in a Markdown file. blyte sees only this file and the repository, nothing
from your conversation. Include:

- **Goal**, in one or two sentences, and **why**.
- **Files** to create or change.
- **What to build**: names, signatures and behaviour that are part of the contract; exact code
  where it matters.
- **Tests** that prove it, written out, and the command that runs them.
- **Constraints**, e.g. "existing tests pass unmodified".
- **The exact commit message.**

Before dispatching, check that the tests fail on the current code and can pass with the intended
change.

## 2. Dispatch

From the repository's main checkout, in the background (you are notified when it exits):

```bash
blyte dispatch T1 /abs/path/task-T1.md --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

- Each line of the gate is a step; all must pass. Put the slow full test run last.
- `--base <branch>` picks the starting commit; independent tasks (disjoint files) can run in
  parallel under different ids.
- The implementer model's API key must be in the environment; blyte never prints it.

It prints one JSON object: `outcome` (`verified`, `no_changes`, `gate_failed`,
`budget_exhausted`, `error`), `exit` (0 only for `verified`), `commits`, `changed_files`,
`final_message` (blyte's own report), `gate_failures`, `turns`, `cost_usd`, `seconds`, and the
`log` and `report` paths. `no_changes` means the gate passed on an untouched tree: not done.

## 3. Verify. Do not trust the summary

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

## 4. Merge

```bash
git merge --ff-only wip/T1        # or cherry-pick if the branch moved
git worktree remove .wt/T1 && git branch -d wip/T1
```

Re-run the checks after merging parallel tasks together. Push or open a pull request only if the
user asks.

## Rules

- One task per dispatch; never hand over a whole plan.
- blyte never pushes, merges or reads secrets. If its log shows otherwise, stop and tell the user.
- Report to the user what **you verified**, not what blyte claimed.
