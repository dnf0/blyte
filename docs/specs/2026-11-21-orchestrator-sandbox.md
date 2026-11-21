# Sandbox the orchestrator's shell, and stop it pushing

Date: 2026-11-21. Status: draft for the user's decision. Nothing here is planned or built yet.

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

## Decisions asked for

1. **The orchestrator's shell is sandboxed like an implementer's**, with the repository's main
   checkout as its root: writes only there (and `.wt/`, the git directory and temporary
   directories), no credential reads, no network. `--sandbox off` remains for a user who wants the
   old behaviour, and the start line says which is in force.
2. **blyte's own commands become a tool.** A `blyte` tool takes a list of arguments and runs the
   blyte binary on the host, with no shell in between. It accepts only `dispatch`, `wait`, `status`,
   `review`, `plan`, `init` and `clean`; anything else is refused with a message that names the
   list. `dispatch` can be started in the background so that several run at once, and `wait` works
   as now. The reference describes this one way of dispatching; `--dispatch-cmd` and its tool go.
3. **Pushing is a tool with the user's yes, not a shell command.** The shell cannot push: it has no
   network and no credentials, which the operating system enforces. A `push` tool pushes the current
   branch and can open a pull request for it. It refuses the default branch, never merges a pull
   request, and runs only after the user confirms that call on the surface. In a one-shot run nobody
   can confirm, so it is refused unless the run was started with `--allow-push`.

Decision 3 is the one with a real alternative: leave pushing out entirely and have the user push by
hand. That is simpler and safer, but the user's own rule is that every change goes in by pull
request, so a surface that cannot open one sends them back to another tool for every change.

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
4. How the surface (PR #377) asks the user a yes-or-no question mid-run, which the `push` tool needs.

## Out of scope

- The implementer's sandbox.
- Approval prompts for anything other than pushing.
- Windows.
