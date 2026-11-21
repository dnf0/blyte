# Approval on the surface

Follows `docs/specs/2026-11-21-orchestrator-sandbox.md` and replaces the outline of Task S4 in
`docs/plans/2026-11-21-orchestrator-sandbox.md`.

On the surface, a command the orchestrator wants to run outside the sandbox is refused today
(`NoApprover` in `AgentRunner::new`) unless the user's allow file already covers it. This plan lets
the user approve or refuse it on the surface.

**The constraint that shapes it.** The model must not be able to approve its own command. On macOS
the orchestrator's sandbox allows connections to `localhost`, and the surface's `POST` endpoints
accept a request with no `Origin` header, so a plain approval endpoint could be called by the model
with `curl`. Hence the secret in S4a.

### Task S4a: a per-start secret on every POST to the surface

**Goal.** Only the page the user opened can change the surface's state. Why: the sandboxed
orchestrator can reach `127.0.0.1`, and must not be able to send messages, change the model, or
(after S4b) approve commands on the user's behalf.

**What must be true when it is done.**
- When the surface starts it generates a secret from the operating system's randomness (at least
  128 bits). It lives in the server's memory only. It is never written to the session log, the
  events, any file, any environment variable or argument of a child process, or any `GET` response.
- The URL blyte opens in the browser, and the URL it prints, carry the secret in the fragment
  (`#token=<secret>`), which browsers do not send to the server.
- The page reads the secret from the fragment, keeps it for the tab (so a reload still works),
  removes the fragment from the address bar, and sends it in a request header on every `POST`.
- Every `POST` the surface serves requires the header with the right value, compared in constant
  time. Missing or wrong: `403`, and nothing changes. `GET` endpoints are unchanged.
- A page opened without the secret still shows everything, and says plainly that it cannot send
  until it is opened from the URL blyte printed. No fallback: there is no flag, environment
  variable or setting that turns the check off.
- The existing `Origin` check stays.

**Tests** for: a `POST` to each existing endpoint without the header, and with a wrong value, is
refused and changes nothing; with the right value it works as before; the secret does not appear
in `GET /api/session`, the events stream, the served page or the session log file; two starts give
different secrets.

**Constraints.** Existing tests keep their meaning; those that `POST` send the secret. No new
dependency unless the workspace has no source of OS randomness already.

**Depends on:** none. **Sibling:** S4b (the approval question) is not in this worktree; do not add
an approval endpoint here.

**Commit message:** `feat(surface): require a per-start secret on every POST`

### Task S4b: approve or refuse an out-of-sandbox command on the surface

**Goal.** When the orchestrator on the surface asks to run a command outside the sandbox and the
user's allow file does not cover it, the surface shows the question and the tool call waits for the
user's answer. Why: the user's rule is that nothing leaves the sandbox without their say-so; today
the surface can only refuse.

**What must be true when it is done.**
- The surface's approver (replacing `NoApprover` in `AgentRunner::new`, keeping the user's allow
  rules in front of it) records a pending approval: an id, the exact command text and the model's
  reason. It appears in the session's events and the session log.
- The tool call blocks until that id is answered. There is no timeout and no default answer.
  Stopping the run, or the surface shutting down, ends the wait as `Approval::Denied`.
- `POST /api/session/approvals/<id>` with body `{"answer":"yes"}` or `{"answer":"no"}` answers it
  (`Approved` / `Denied`). Any other body: `400`. An unknown id: `404`. An id already answered:
  `409`, and the first answer stands. The endpoint is covered by S4a's secret like every `POST`.
- An answer applies to that one command, once. There is no "approve all", no remembered answer, and
  no way to answer by sending a chat message.
- The surface shows the pending question prominently, with the command verbatim as text (never
  interpreted as HTML), the reason, and two buttons. After the answer it shows which was given.
  The answer is written to the session log with the command.
- Terminal sessions (`TtyApprover`) and one-shot runs (refused) are unchanged.

**Tests** for: `ask` blocks until answered; yes gives `Approved`, no gives `Denied`; stopping the
run gives `Denied`; a second answer to the same id is refused and does not change the first; an
unknown id and a malformed body are refused; a command covered by the allow file never creates a
pending approval; the pending approval and its answer appear in the events with the exact command.

**Depends on:** S4a.

**Commit message:** `feat(surface): approve or refuse an out-of-sandbox command on the surface`
