# One run by default

Proposed on 2026-11-22, after the measurements in `docs/evidence.md` ("The orchestrator: default
process, corrections, and no orchestrator" and "A request with several tasks"). **Not yet agreed:**
the four decisions below are the user's.

## What the evidence said

- On a one-task request, an orchestrator that passes the request on cost USD 0.5 to 1.4 more than
  giving the request straight to one run.
- On a three-task request, splitting it across two to four runs cost USD 5.15 to 9.16 and 36 to 75
  minutes over six trials; one run cost USD 4.81 to 5.39 and 33 to 40 minutes. Every result passed
  the full check either way.
- The orchestrator's cost is nearly all after it dispatches: it stays in its turn, waits, reads
  results, and each turn re-sends the conversation. Once it also redid a run that had passed.
- In both measurements "one agent" was one `blyte dispatch` of the request as the user wrote it: a
  fresh run in its own worktree, ended by the repository's check. An agent that edits inside the
  conversation the user is having has not been measured.

## Proposal

A request for work on the surface becomes **one run**, and no model waits for it.

1. The agent the user talks to answers questions itself. For work it makes one call that hands the
   request on as written, and its turn ends there.
2. blyte's code, not a model, follows the run: when it ends, the surface shows the result, and the
   existing steps apply (the review checks, the combine and its full check, **Push branch** and
   **Open pull request**).
3. A follow-up to a request that has not been pushed starts a new run from that request's branch.
4. The agent splits work across several runs only when the user asks for it, or when a run stops
   because it ran out of budget and the user agrees to a split.

## Decisions for the user

| | Question | Recommendation |
|---|---|---|
| 1 | Does the agent the user talks to hand work to one run (as above), or edit the code itself inside the conversation, as Claude Code does? | Hand to one run. It is what was measured, it keeps the conversation small, and everything in #400 applies unchanged. |
| 2 | When a run ends, does a model read the result and summarise it, or does the surface show the run's own report? | Show the run's own report. A model turn only when the run failed or the user asks. |
| 3 | When is work split across runs? | Only when the user asks, or after a run exhausts its budget and the user agrees. Never on the agent's own judgment. |
| 4 | Is "orchestrator" still the name? | No. With this it only answers and hands on; the instructions for splitting and re-dispatching shrink to the case in decision 3. |

## Not covered

- Work too large for one run has not been measured; decision 3 leaves splitting available for it.
- A cheaper model for the agent the user talks to.

## Measurement

After it is built: the same three-task request through the surface, three trials, against the
three one-run trials already recorded. The surface should cost what one run costs plus one or two
model turns.
