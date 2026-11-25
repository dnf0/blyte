# Independent surfaces that can message each other

Date: 2026-11-25. Status: decisions agreed with the user; not yet planned.

## Why

Today a checkout has one surface. A second `blyte` start in the same checkout opens the
first one's page and exits, even with `--port`. The user works in several windows on one
repository, each on a different piece of work, and wants those windows to tell each other
what they are doing, the way separate Claude Code sessions do.

## Decisions

1. **Any number of surfaces per checkout.** Each `blyte` start is its own process with its
   own port, token, conversation and browser tab or window.
2. **Each surface sees its own requests and runs.** A finished run reports back to the
   surface that dispatched it.
3. **A registry of running surfaces on the machine.** A `surfaces` tool lists them and a
   `tell` tool messages one by name, across repositories too.
4. **Peer messages are information only.** They carry a message-only credential and can
   never approve, push or open a PR. A surface's push and PR buttons act only on its own
   requests.

Rejected: one surface per checkout holding several conversations in tabs. It kept one
server and one token per repository, which was convenient to build, but it is not how the
user works.

## What is true today (v0.63.2)

Checked against the code; file names are in `crates/blyte-cli/src/`.

| Fact | Where |
|---|---|
| A second start probes the port in `<root>/.wt/surface.json`, opens that surface and exits 0. `--port` is not consulted. | `serve.rs`, `run_inner` |
| One token per repository, at a path derived from the main checkout. | `serve.rs`, `sandbox` |
| One transcript per repository: `<main checkout>/.wt/sessions/entries.jsonl`. Only this survives a restart; the model's message history does not. | `serve.rs`, `session.rs` |
| A run's start record `.wt/<id>.start.json` already holds `"request"` and `"session"`. Nothing reads `"session"`. | `dispatch.rs`, `run_results.rs`, `requests.rs` |
| A finished run notifies the surface named in `<main checkout>/.wt/surface.json`. That surface then reports every unreported run in the checkout that has a request. | `surface_notify.rs`, `run_results.rs` |
| The approver, write scope and session id are process-wide. With one process per surface this is not a problem. | `blyte-agent/src/tools.rs` |
| Machine-wide state already has a home: the directory `registry::state_dir()` returns. | `registry.rs` |
| `--port 0` already picks a free port. | `cli.rs` |

**A defect this design removes.** In a linked worktree, `surface.json` goes in the
worktree's `.wt/` but the token and transcript go in the main checkout's. A surface started
in a linked worktree overwrites the token of the surface in the main checkout, deletes it on
exit, and appends to the same transcript. Traced in the code, not reproduced. It is fixed on
its own first (see Order of work), because it is live today.

## Design

### A surface

Each start creates a surface with:

- an **id**, random, never reused;
- a **name**, used by people and by `tell`. Default: the repository's directory name, then
  `<name>-2`, `<name>-3` for the next free one among running surfaces on the machine.
  `--name <name>` sets it. A name already in use by a running surface is an error.
- its own directory, `<main checkout>/.wt/surfaces/<id>/`, holding `surface.json` (id, name,
  pid, port, started_at), `token`, `entries.jsonl` and its `tasks/`.

The sandbox rule that keeps models away from the surface token covers every file named
`token` or `message-credential` under `.wt/surfaces/` and under the registry.

`--port` now means what it says: that port, or an error if it is taken. Without `--port`,
the surface takes a free port.

`blyte` always starts a new surface. To get back to a running one, `blyte surfaces` lists
them and `blyte open <name>` opens its page. `blyte --resume <name>` starts a surface from a
stopped surface's directory, keeping its id, name and transcript. As today, the model's own
history is not restored; persisting it is a separate piece of work.

The old `.wt/surface.json` and `.wt/sessions/entries.jsonl` are not read any more and not
migrated. `blyte clean` removes them, and removes stopped surface directories after 14 days,
the same age limit as runs. It never touches a running surface's directory.

### Runs belong to a surface

- The surface passes its id to every `blyte` command its model starts. The start record
  stores it as `"surface"`.
- A finishing run notifies the surface named in its own start record, not "the" surface of
  the checkout.
- A surface reports, combines and lists only runs and requests whose `"surface"` is its own
  id. Its push and open-PR endpoints refuse a request that belongs to another surface.
- A run with no surface (dispatched from a terminal or another harness) belongs to none, as
  now. `blyte dashboard` still shows every run.
- If the owning surface has stopped when a run finishes, the run stays unreported until that
  surface is resumed. A start record naming a surface whose directory does not exist is an
  error shown by `blyte dashboard`, not a run quietly given to someone else.
- Run ids stay per checkout. A second surface dispatching an id that exists gets today's
  "already exists" error.

### The registry

One file per running surface in `<state dir>/surfaces/<id>.json`: id, name, repository
root, branch, pid, port, started_at, and what it is doing (idle or the current request's
first line). Next to it, `<id>.message-credential`, mode 600.

A surface writes its entry on start, updates it when a request starts or ends, and removes
it on exit. A reader checks each entry by asking the port for its health and comparing the
id; an entry that fails is stale, is left out of the list and is removed.

### `surfaces` and `tell`

Both exist as tools for the conversation model and as commands (`blyte surfaces`,
`blyte tell <name> <message>`), so the user and other harnesses can use them too.

- `surfaces` returns name, repository, branch and what each is doing.
- `tell` posts to the target's `POST /api/peer/messages` with the target's message
  credential and the sender's name. It returns `delivered` or `held` (see below). An unknown
  name, a stale entry or a refused connection is an error that says which.

The message credential is separate from the surface token. The peer endpoint accepts only
the message credential; every other endpoint refuses it. So the most a holder can do is put
a message in a queue. Its purpose is to stop web pages and stray local processes injecting
messages. Implementer runs are sandboxed away from it, so only conversation models and the
user can send.

### What a peer message can do

- It is shown in the target's page as "from surface `<name>` (`<repository>`)", and given to
  the model marked as information from another surface, not from the user.
- It is delivered between turns, never into a turn in progress. If the surface is idle it
  starts a **peer turn**.
- In a peer turn the model can read, answer the user in the page, and `tell` a reply. It
  cannot start a run or edit a file: the tool layer refuses, with an error saying a peer
  message cannot start work. To act on a peer message, the user says so.
- Approvals, push and open-PR stay behind the surface token, which only the page holds. No
  peer message, and no model, can reach them.
- To stop two models talking in a loop, a surface runs at most five peer turns between user
  messages. Further messages are queued and shown in the page, and `tell` returns `held`.

A limit to be clear about: once a peer message is in the conversation, later turns the user
starts can see it, as they can see any file the model has read. The marking is the defence
there, as it is for file contents.

## Out of scope

- Persisting the model's message history across a restart.
- Surfaces on different machines.
- One surface driving another (starting runs, approving) on its behalf.

## Order of work

1. Fix the linked-worktree token defect on its own: resolve the surface's root to the main
   checkout, with a regression test. Small, and independent of the rest.
2. Surface id, per-surface directory, free port, always-new start, `--name`, `--resume`,
   `blyte clean`.
3. Run ownership: `"surface"` in the start record, notify and reconcile by owner, endpoints
   refuse other surfaces' requests.
4. Registry, `blyte surfaces`, `blyte open`.
5. Message credential, peer endpoint, `tell`, peer turns and their limits.

Steps 2 and 3 ship together: several surfaces without run ownership would each report every
run. Step 5 depends on 4.

## Checks before planning

- How the page and `surface_notify` find the token today, so every reader moves to the
  per-surface path in one change.
- Whether anything else reads `.wt/surface.json` (the dashboard, `blyte wait`, the skill).
- That appends to `usage.jsonl` from several processes are one write per line.
- How a peer turn is told apart in the tool layer: where the run and edit tools can see the
  turn's origin.
