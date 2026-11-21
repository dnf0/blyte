# blyte V1: the live surface, first version

Date: 2026-11-21. Follows `docs/specs/2026-11-20-blyte-v1.md` (step 2 of its order of work).
The facts below were checked against the code by a research run on 2026-11-21.

## Goal

Typing `blyte` in a repository opens the surface on a live orchestrator session: the developer
talks to the orchestrator in the page, dispatched runs appear in the review queue as they
happen, and decisions made in the queue are real. It must work on Google models alone.

## Decisions

### 1. Launch

- Bare `blyte` in a git repository starts the session server for that repository, opens the
  surface in its own window, and stays in the terminal printing the address until interrupted.
- A second `blyte` in the same repository opens a window on the running session; it does not
  start another.
- `blyte --terminal` starts the existing terminal session. `blyte "<prompt>"`,
  `blyte --orchestrate`, `blyte dispatch` and the other commands are unchanged. `blyte mock`
  stays, for replaying recorded runs.
- `blyte --resume` reopens the last session of the repository. It depends on saved sessions
  (the session-resume work) and is not part of this first version.

### 2. The window

The first version opens the page in the developer's browser, in app mode where the browser has
one (a window with no tabs or address bar). The desktop shell in the V1 spec comes later and
will be compiled into the `blyte` binary, so there is one thing to install and one command. The
page and the server are the same in both.

### 3. The interface is blyte's own, over local HTTP

The server listens on `127.0.0.1` only. The Agent Client Protocol named in the V1 spec is
deferred: it has no background runs or review queue, so most of the surface would be extension
methods either way. The session log's entries are shaped so they can be mapped to that
protocol's session updates later.

| Request | Meaning |
| --- | --- |
| `GET /` | the surface |
| `GET /api/session` | session id, repository, model, effort, whether a turn is running, cost totals |
| `GET /api/session/events?after=<seq>&wait_ms=<n>` | entries of the session log after `seq`; waits up to `wait_ms` when there are none |
| `POST /api/session/messages` | `{text, reports?, focus_group?}`: a message from the developer |
| `GET /api/models` | models the configured keys can use, and which accept an effort |
| `POST /api/session/model` | `{model}` |
| `POST /api/session/effort` | `{effort}`: `low`, `medium` or `high` |
| `GET /api/runs` | the repository's runs: running, finished, held, with their checks |
| `GET /api/runs/<id>/diff` | the diff of a code run |
| `GET /api/runs/<id>/report` | the report of a research run |
| `POST /api/runs/<id>/decision` | `{action, note?}`: `accept`, `send_back`, `redo`, `hold`, `mark_read` |
| `GET /api/map` | the system and data map of the repository |

### 4. The session log

Every view follows one log of numbered entries, read from a sequence number, so a view that
reconnects misses nothing. There is no separate event stream. Views wait for new entries by
long polling; no new dependency is needed.

Entry kinds: `user_message`, `turn` (the orchestrator's text and token use), `tool_started`,
`tool_finished`, `retry` (a model call being retried, with the reason), `error`, `idle` (the
orchestrator has stopped and is waiting, with the session's cost so far), `model_changed`,
`effort_changed`, `decision` (a queue decision, with the run and note).

The orchestrator's text arrives whole, one turn at a time. Token-by-token text is not part of
this version.

### 5. Messages while the orchestrator is working

A message sent while a turn is running is accepted and queued; it is delivered when the current
turn ends. The page shows it as waiting. Interrupting a running turn is not part of this
version.

### 6. Model and effort

Both are chosen in the surface and take effect from the next message. Effort is offered only
for models whose provider accepts it (Gemini and Vertex today); for other models the control is
disabled and says why. Choosing a model or effort that the provider rejects is an error shown in
the conversation; nothing is substituted.

### 7. What the queue's decisions do

- **Checks are computed when a run finishes**, by the engine, so a run with nothing flagged
  can be shown as clean at once. The orchestrator still reviews first and reports what it found.
- **Accept** records the verdict `accepted` and tells the orchestrator, which combines the run
  onto the working branch and runs the gate, as it does today. The server does not run git
  itself in this version.
- **Send back** needs a note. The note goes to the orchestrator, which drafts a follow-up task
  for the developer to confirm in one step.
- **Redo** records the verdict `redone` and tells the orchestrator.
- **Hold** needs a note, saved with the run so it survives a restart. The run stays undecided.
- **Mark read**, for a research run, records the verdict `accepted`.

### 8. Reliability comes first

A model call must fail with an error within a bounded time, and the session must record its
cost each turn. Both are planned separately and land before the surface is relied on.

## Not in this version

Saved and resumed sessions; interrupting a turn; token-by-token text; the desktop shell; the
Agent Client Protocol; several repositories in one window; load limits and check-ins on live
data (they stay as in the mock until its trial sets the numbers).

## Open

- The name of the `--terminal` flag.
- Whether Accept should combine the run itself, without the orchestrator, once the session has
  a named working branch.
- Whether a research run needs its own verdict (`read`) and its own waiting count.
