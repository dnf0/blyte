# A thin orchestrator

Agreed with the user on 2026-11-21, after the trial in `docs/evidence.md` ("The orchestrator:
default process, corrections, and no orchestrator") and two requests driven through the surface.

## What the evidence said

- A detailed task written by the orchestrator gave no better result than the request passed on as
  it was, and cost USD 1 to 3.7 more per run. Passing the request on (#390) cut the orchestrator's
  own cost from about USD 3 to about USD 0.65 on the same task.
- On the surface the orchestrator still cost as much as the implementer (USD 2.70 against 2.48),
  nearly all of it after dispatching: reviewing, re-running checks the run's gate had already
  passed, cherry-picking and committing, each turn re-sending the whole conversation.
- Everything measured so far is one change per request. A request that splits into several tasks
  has not been measured.

## Decision

The orchestrator is the thing the user talks to, and nothing more. It answers, or it hands the
request on. The other jobs it has been doing move to where they belong.

| Job | Where it goes |
|---|---|
| Holding the conversation; answering questions; deciding whether to dispatch and how the work splits | The orchestrator |
| Working out what the work actually is (already done, smaller than stated) | The implementer, which reads the code anyway, in its report |
| Combining passed runs and running the check on the result | blyte's code, automatically |
| Pushing a branch; opening a pull request | The user, with a button on the surface |
| Review | blyte's built-in checks first; a fresh run that sees only the task and the diff when they flag something or the change is risky |

## Combining

- The runs dispatched for one request form a group.
- When every run in the group has passed, blyte creates a work branch from the group's base commit,
  applies each run's commits in dependency order (`git cherry-pick -x`, so the runs still count as
  merged), and runs the repository's full check on the result. No model takes part and the user
  does nothing.
- If two runs conflict, or the check fails on the combined branch, nothing is hidden and nothing is
  skipped: the surface shows which runs and the failing output, and the orchestrator is told, so it
  can dispatch a fix as an ordinary task. blyte does not resolve conflicts itself.
- Combining is resumable and idempotent: it can be run again for the same group and gives the same
  branch; a group already combined is not combined twice.

## Leaving the machine

- When a group is combined and its check has passed, the surface shows the branch, its commits and
  two buttons: **Push branch** and **Open pull request**.
- The buttons are `POST`s that carry the surface's per-start secret
  (`docs/plans/2026-11-21-surface-approval.md`, Task S4a), so only the page the user opened can
  press them. The model cannot.
- blyte refuses to push the repository's default branch, and never merges a pull request.
- The model no longer pushes or opens pull requests as part of ordinary work. The approval question
  for an out-of-sandbox command (Task S4b) stays, as the route for the rare command that is neither
  a push nor a pull request.

## The implementer's report

An implementer says in its report when the request was already satisfied, or was smaller or larger
than stated, and what it did instead. The orchestrator does not read the code to find this out
first.

## Review

- `blyte review <id>`'s checks run for every finished run, as now.
- A fresh review run, given only the task and the diff, is started when those checks flag something
  or the change touches a risky area (security, data handling, migrations, a public interface). It
  is not started for a run whose checks flag nothing.
- The orchestrator does not re-run a check that a run's gate has passed, and does not read diffs.

## What is not decided here

- Whether a single change should skip the handoff altogether and be done by the agent the user is
  talking to. To be measured: the thin orchestrator against one agent doing the work directly, on a
  request that splits into several tasks.
- Whether the orchestrator can run on a cheaper model than the implementer.

## Measurement

After this is built: three runs each of the thin orchestrator and of one agent working directly, on
a request with at least three tasks, two of them independent. Compared on total cost, time, and
whether the combined result passes the check.
