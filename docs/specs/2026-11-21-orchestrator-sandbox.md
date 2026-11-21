# Sandbox the orchestrator's shell, and stop it pushing

Date: 2026-11-21. Status: decisions agreed with the user on 2026-11-21; the facts below are still to be checked before planning.

## The problem

An implementer's shell is sandboxed: it can write only inside its worktree and temporary
directories, it cannot read credentials, and it has no network. The orchestrator's shell has none of
that. `crates/blyte-cli/src/task_runner.rs:97` sets `sandbox` to `off` whenever `--orchestrate` is
given, so the model that plans and merges can write anywhere the user can, read every credential,
and run `git push` or `gh pr merge`. The only thing against it is a sentence in its instructions
("Change nothing outside the repository yourself"; "Push or open a pull request only when the user
asks").

That was a decision, recorded on 2026-10-07 (`docs/superpowers/specs/2026-10-07-orchestrate-mode.md`,
lines 34-36): the orchestrator's shell gets "the same trust Claude Code has". This spec reverses it.
The reason to reverse it: in V1 the orchestrator is the model the user talks to all day, on a
cheaper model than the one that decision assumed, and one wrong command from it does more damage
than one from an implementer.

## Why it cannot simply be switched on

Research run `R-orch-sandbox` (2026-11-21) read the code for what breaks. With the sandbox on, the
orchestrator's shell could not do its own job:

| What the orchestrator runs | Why it fails in today's sandbox |
|---|---|
| `blyte dispatch` | The implementer is started inside its own sandbox, and macOS refuses `sandbox-exec` inside `sandbox-exec` (exit 71). On Linux, Landlock allows stacking but the inner run could read only what the outer one could. |
| `blyte dispatch` | It needs the model's API key (`~/.config/blyte/env`, or gcloud or Codex credentials), and the sandbox exists to keep those from the shell. |
| `blyte dispatch`, `blyte review` | They reach the model provider, and the sandbox has no network. |
| `blyte dispatch`, `blyte clean` from a linked worktree | They write the main checkout's `.wt/`, outside the sandbox's root. |

So the orchestrator's `blyte` commands have to run outside the model's shell. One mechanism does that
already: with `--dispatch-cmd`, dispatch is a tool the harness runs on the host, not a shell command
(`crates/blyte-agent/src/tools.rs:900`). It covers only `dispatch`, one at a time.

Nothing in the code blocks a push today. Implementers cannot push only because their sandbox hides
the credentials and the network; `blyte review` flags `git push` in a log after the fact.

## Decisions

1. **The orchestrator's shell is sandboxed like an implementer's**, with the repository's main
   checkout as its root: writes only there (and `.wt/`, the git directory and temporary
   directories), no credential reads, no network. `--sandbox off` remains for a user who wants the
   old behaviour, and the start line says which is in force.
2. **blyte's own commands become a tool.** A `blyte` tool takes a list of arguments and runs the
   blyte binary on the host, with no shell in between. It accepts only `dispatch`, `wait`, `status`,
   `review`, `plan`, `init` and `clean`; anything else is refused with a message that names the
   list. `dispatch` can be started in the background so that several run at once, and `wait` works
   as now. The reference describes this one way of dispatching; `--dispatch-cmd` and its tool go.
3. **A command leaves the sandbox only with the user's yes.** The model uses ordinary commands
   (`git push -u origin <branch>`, `gh pr create …`), as in other harnesses. The `bash` tool takes
   an `unsandboxed` flag with a one-line reason. Such a command does not run until the user has
   approved that exact text on the surface; a refusal is returned to the model as an error. In a
   one-shot run nobody can approve, so it is always refused. No flag lets an unattended run push.
4. **The user can allow commands in advance.** The file `~/.config/blyte/allow-unsandboxed` (the user's own, not the repository's `blyte.toml`,
   which the model can edit) lists command prefixes, one per line, that run without a question, for example `git push -u origin`. A rule matches
   only one simple command: anything with a pipe, `;`, `&&`, a substitution or a redirect is always
   asked about. There are no built-in rules.

Agreed with the user on 2026-11-21, after considering two alternatives: no pushing from blyte at all,
and special tools (`/pr`, a `propose_pr` tool) in place of ordinary commands. The second was dropped
because models are trained on `git` and `gh`, and one general mechanism also covers anything else
that needs the host. A third, leaving the shell unsandboxed as Pi does, has the least friction and
no protection.

Outside blyte, and recommended whatever is built here: protect the default branch on GitHub, so that
a direct push or an unreviewed merge is refused by the server. An allow rule cannot tell `main` from
another branch reliably; the server can.

## What this does not stop

- A command the model runs inside the repository: deleting files, rewriting history on a local
  branch, a bad merge into the local default branch. Git and the reviews are the protection there.
- The `blyte` tool's own reach: `blyte clean` removes worktrees, and an implementer's task text is
  whatever the orchestrator writes.
- A check on command text would not be added. The research found it stops accidents only (a script
  or `sh -c` passes it), and the sandbox already does the real work.

## Facts to check before planning

1. Whether the repository's full check can run inside the sandboxed shell. Tests here start
   sandboxed child processes, which would hit the same macOS refusal; if so the combined check needs
   to run on the host too (through the tool, as the gate does for implementers).
2. What the package caches need: a check that downloads dependencies has no network in the shell.
3. That `git cherry-pick`, `git merge` and `git commit` in the main checkout and in a linked
   worktree work inside the sandbox on macOS and Linux.
4. Whether the surface (PR #377) can ask the user a yes-or-no question while a run is going, and
   what a conversation without the surface (`blyte --orchestrate` at a terminal) can do.
5. Where user-level configuration is read today, and whether `~/.config/blyte/config.toml` exists.

## Out of scope

- The implementer's sandbox.
- Approval prompts for commands inside the sandbox.
- Windows.
