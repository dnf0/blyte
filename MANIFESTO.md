# The Blyte Manifesto

Fast and good. No bunk.

A coding agent is a model plus a harness. The model does the work. The harness decides how much
the work costs, how long it takes, and whether "done" means anything. Blyte is a harness built on
that division, and on what measuring it taught us.

## 1. The model does the work; the harness decides the bill

Holding the model fixed and changing only the harness moves cost by up to 40× and pass rate by a
few points (*The Scaffold Effect*, 2607.22585). Our own numbers say the same. On eighteen real
plan tasks in two repositories, blyte and agy running the same model produced work of comparable
quality, and blyte did it in a median 0.49× the time and 0.63× the cost. It was cheaper every time.

A harness that makes the model smarter is rare. A harness that wastes the model's time is
everywhere. Blyte's job is to get out of the way.

## 2. The turn is the unit of cost

Every turn re-sends the prompt. One turn costs as much as 100–200K cached tokens, and cost per turn
is steady, so the runs that cost more are the runs that took more turns. Every change to blyte is
judged by the turns it saves:

- `read` returns a whole file at once, because paging through one 650-line file cost 15 turns.
- A read from line 1 of a file that fits comes back whole, because models page out of habit.
- The gate reports every failing step together, because `fmt && clippy && test` let a second
  failure hide behind the first and cost a whole extra round.

Turns are not the only cost. A gate run is a full test suite, so a gate failure the model could
have caught in seconds is the most expensive mistake it can make. Blyte asks it to run the quick
checks itself.

## 3. The environment decides truth

The model does not decide when it is finished. A command does: `--gate` exits 0 or the work is not
done. The gate is not in the model's tool surface, so it cannot be skipped, run selectively, or
reported on by the thing being graded. A failed attempt can be rolled back to the last verified
commit, so failures do not pile up in the tree.

Confidence is not evidence. An exit code is.

## 4. Every feature earns its place, or it is deleted

Blyte once had a daemon, an MCP server, a worktree engine, advisory review, a TDD mode, a cost cap,
journal and resume, a loop-breaker, a working set, an action ledger, a note tool, LLM-written fold
notes and four context strategies. They were plausible. They did not win on turns, cost or
completion in repeated runs, so they were removed.

What is left is about 5,600 lines of Rust: one loop, a sealed gate, folded context, bounded
output, provider retry, persistent shell sessions and in-process search. A new feature has to beat
that in measurement, or it does not stay.

## 5. Mechanical beats clever

When the prompt grows past a budget, the oldest turns fold into a one-line digest per tool call.
No model summarises them, because summaries did not pay for themselves, and a tool for recovering
elided content goes unused (2609.20804). Tool output is cut by rule: 500 characters a line, 60K a
result, head and tail kept. Rules are cheap, predictable and testable. Cleverness is none of those.

## 6. The harness's own time is overhead to eliminate

Commands run in a persistent shell session, at about 0.7 ms of harness time each, where a fresh
`docker exec` cost 60–110 ms. `grep` and `glob` run in-process on ripgrep's libraries. Across 200
turns the harness now spends about 2 s of its own time locally. The model's time is the bill; the
harness's time should round to zero.

## 7. Do one job

Blyte implements. It takes a task, does it and proves it with the gate. Deciding what to build,
splitting the work and judging whether the result is complete belong to whoever is orchestrating,
whether that is a person or another agent. That is why blyte has no review mode, no planner and no
memory across runs. A harness that tries to do everything does each thing worse and costs more
while it does it.

## 8. Measure what you claim

"Faster" means faster on the same task, from the same commit, run together with the thing you
compare against. Blyte is developed with paired shadow runs: each change is implemented by blyte
and by agy on the same task, both results are checked, and every run goes into a ledger. Only
paired runs count as comparisons. Everything else is an anecdote, and we have been caught treating
an anecdote as evidence before.

A number without a paired run behind it is a guess.

---

The measurements behind these claims are in `docs/research/2026-09-24-harness-research.md` and the
superstate-eval findings.
