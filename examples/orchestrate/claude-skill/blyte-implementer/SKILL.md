---
name: blyte-implementer
description: Use when a task can be handed to a cheaper model to implement while you plan and review: write a precise task, dispatch it to headless blyte in its own git worktree with a check command as its gate, then verify the result yourself and merge it. Use for well-specified changes in a git repository with a runnable check (tests, lint).
---

# blyte as your implementer

You plan and review; blyte implements, on a cheaper model, and its gate (the repository's check
command) decides whether the work is done. Your job is to make the task unambiguous and to verify
the result, not to trust it.

Scripts (from the blyte repository's `examples/orchestrate/`; adjust the path to where they are
installed):

```bash
DISPATCH=~/.claude/skills/blyte-implementer/blyte-dispatch.sh
REVIEW=~/.claude/skills/blyte-implementer/blyte-review.sh
```

## When to delegate

Delegate when the task is **specified well enough that a careful junior engineer could do it
without asking you anything**, and the repository has a check that would catch a wrong result.
Keep for yourself: design decisions, ambiguous requirements, anything with no check, and anything
touching credentials, deployment or other repositories.

## 1. Write the task

One task per dispatch, in a Markdown file. blyte sees only this file and the repository, nothing
from your conversation. Include:

- **Goal**, in one or two sentences, and **why** (a line of context prevents literal-minded mistakes).
- **Files** to create or change.
- **What to build**: names, signatures and behaviour that are part of the contract. Where exact
  code matters (a test, an interface), give it verbatim.
- **Tests** that prove it, ideally written out, and the command that runs them.
- **Constraints**: what must not change ("existing tests pass unmodified").
- **The exact commit message.**

Before dispatching, check that the tests you give fail on the current code and can pass with the
intended change.

## 2. Dispatch

From the repository's main checkout, in the background (you are notified when it exits):

```bash
BASE=<branch> $DISPATCH T1 /abs/path/task-T1.md 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace'
```

- Each line of the gate is a step; all must pass. Put the slow full test run last.
- Independent tasks (disjoint files) can run in parallel under different ids.
- The model's API key must be exported; the script never prints it.

The script prints a JSON summary: `exit` (0 = gate passed and something was committed; 4 = passed
but nothing committed), `outcome`, `gate_failures`, `turns`, `cost_usd`, `seconds`.

## 3. Verify. Do not trust the summary

```bash
$REVIEW T1 --diff
```

Check, in order:

1. **Scope**: only the files the task names changed, in the commit(s) you asked for, with the exact message.
2. **Fidelity**: code you gave verbatim landed verbatim; read everything else in full.
3. **Checks yourself**: run the gate in the worktree (`.wt/T1`). A passing gate is necessary, not sufficient.
4. **Deviations**: every deviation in blyte's report is justified, and there are none it did not report.
5. **Out-of-scope effects**: skim the log for commands that touched anything outside the worktree.

If it falls short: fix a trivial issue yourself, or write a follow-up task saying exactly what to
change and dispatch it under a new id into the same worktree
(`BASE=wip/T1 $DISPATCH T1b ...` creates a fresh worktree from that branch). If the task was
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
