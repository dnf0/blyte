# blyte as the implementer for a frontier model

A pattern that works well: a frontier model (Claude Code, for example) **plans and reviews**, and
hands each well-specified task to blyte, which **implements** it on a cheaper model. The
repository's check command, run by blyte as its sealed gate, decides whether the work is done. The
frontier model then verifies the result and merges it.

```
 frontier model (orchestrator)            blyte (implementer, cheaper model)
 ─────────────────────────────            ──────────────────────────────────
 write the task, with tests  ───────────▶  implement in .wt/<id>, run quick checks, commit
                                           gate: the repository's check  ──▶ pass / fail + output
 read the report and the diff ◀──────────  report: outcome, gate failures, turns, cost
 merge, or send a follow-up task
```

## Why it works

- **Each model does what it is good at.** Planning, judging ambiguity and reviewing a diff are
  where a frontier model earns its price. Typing out a specified change, running the tests and
  fixing what they report is work a cheaper model does well inside a good harness.
- **The gate makes the cheap model's work checkable.** blyte cannot declare itself done: the check
  passes or it does not, and every failure comes back to the model at once.
- **The report tells the orchestrator what to do next.** `outcome`, `gate_failures`, `turns` and
  `cost_usd` are enough to decide between merge, follow-up and doing it yourself.
- **Runs are contained.** Each task has its own worktree, blyte cannot push or merge, and the
  sandbox keeps commands inside the workspace and away from credentials.

!!! note "What has and has not been measured"
    blyte was developed this way: every change in its own repositories was planned by Claude and
    implemented by blyte on Gemini 3.8 Flash, with results reviewed and merged by Claude. Against
    another harness doing the same tasks on the same model, blyte took about half the time and two
    thirds of the cost ([Evidence](evidence.md)). **The saving against a frontier model doing the
    whole task itself has not been measured yet**: the orchestrator's own planning and review cost
    frontier tokens too. Measure it on your own work before relying on a number.

## When to delegate, and when not to

Delegate a task when a careful engineer could do it from the task description alone, without
asking anything, and the repository has a check that would catch a wrong result.

Keep it with the orchestrator when it needs a design decision, the requirements are ambiguous,
there is no check, or it touches credentials, deployment or other repositories.

In practice the pattern pays off most when tasks are **specified precisely**: the files involved,
the names and signatures that matter, the tests (written out), and the exact commit message. The
more of the thinking is in the task, the less the implementer has to guess.

## The workflow

The blyte repository ships two scripts and a Claude Code skill in
[`examples/orchestrate/`](https://github.com/climate-x-org/superstate/tree/main/examples/orchestrate).

**1. Write the task** to a file, one task per file:

```markdown
## Task: reject empty input in parse()

`parse("")` currently panics. It should return `Err(ParseError::Empty)`.

Files: `src/parse.rs`, `tests/parse_empty.rs` (new).

Test (add verbatim):
    #[test]
    fn empty_input_is_an_error() {
        assert_eq!(parse(""), Err(ParseError::Empty));
    }

Constraints: existing tests pass unmodified.
Commit message: fix(parse): empty input is an error, not a panic
```

**2. Dispatch it** from the repository's main checkout:

```bash
examples/orchestrate/blyte-dispatch.sh T1 task-T1.md 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

The script creates the worktree `.wt/T1` on branch `wip/T1`, runs blyte with the gate (rollback
off, so a failed attempt is kept for review), and prints a summary, for example:

```json
{ "id": "T1", "exit": 0, "commits": 1, "outcome": "verified", "gate_failures": [],
  "turns": 21, "gate_attempts": 1, "cost_usd": 0.18, "seconds": 142, "sandbox": "seatbelt" }
```

`exit` is 0 when the gate passed and something was committed, and 4 when the gate passed on an
untouched tree, which is not done.

**3. Verify it yourself:**

```bash
examples/orchestrate/blyte-review.sh T1 --diff
```

Check scope (only the files the task names), fidelity (code you gave verbatim landed verbatim),
the checks (run the gate yourself in `.wt/T1`), and any deviations blyte reported. A passing gate
is necessary, not sufficient.

**4. Merge or follow up:**

```bash
git merge --ff-only wip/T1 && git worktree remove .wt/T1 && git branch -d wip/T1
```

If the result falls short, fix a trivial issue yourself, or write a follow-up task saying exactly
what to change. If the task was wrong, fix the task.

## With Claude Code

Install the skill, and Claude Code will follow this workflow when you ask it to use blyte:

```bash
mkdir -p ~/.claude/skills/blyte-implementer
cp examples/orchestrate/claude-skill/blyte-implementer/SKILL.md \
   examples/orchestrate/blyte-*.sh ~/.claude/skills/blyte-implementer/
```

Then, in a session: *"Plan this change, then use blyte to implement each task and review the
results."* Claude writes the task files, dispatches them in the background (independent tasks in
parallel), reviews each diff and merges. Your Claude usage goes on planning and review; the
implementation runs on whichever model blyte is configured for.

## Tips from running it

- **Give the tests, not just the goal.** With the tests written out, the implementer knows exactly
  what must pass, and the gate checks the thing you meant rather than what it guessed.
- **Check that the tests can pass.** A test that fails for a reason the change cannot fix (an
  over-long line the tool output clips, a missing fixture) burns every attempt.
- **Name the commit message.** "Use the message in the plan" produced the wrong message in two of
  our first six dispatches; stating it verbatim in the task fixed that.
- **Coupled tasks go together.** If task 2 cannot pass without task 1, dispatch them as one task
  or into one worktree.
- **Read the log for out-of-scope commands**, not just the diff. The sandbox limits the damage;
  review catches the intent.
