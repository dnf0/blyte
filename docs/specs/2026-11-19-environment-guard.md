# Environment guard: stop when the gate cannot run, and flag borrowed environments

Date: 2026-11-19

## Problem

In four dispatches in one repository (2026-11-18), the gate's Python environment could not be
built: the gate ran offline and two packages were not in the cache. blyte handed the worktree to
the model anyway. The model then:

- copied `.venv` directories from the main checkout, from another repository, from a sibling
  worktree and from the uv cache, and patched paths in them with `sed`;
- spent 52 to 124 turns doing so;
- in two runs reached `verified`, on a gate that ran in a copied environment.

Nothing in the task asked for this. A separate report, "one model call waited 47 minutes with no
timeout", was checked against the machine's power log: the Mac entered idle sleep 14 seconds
after the run's last log line. It was not a missing timeout.

## What was verified in the code (at e15e5f3)

| Fact | Where |
|---|---|
| `[dispatch].setup` runs on the host before the model; a failure ends the run as `blocked` | `crates/blyte-cli/src/dispatch.rs:853-895` |
| With no `setup`, nothing checks the environment before the model starts. The gate is run at the base commit only after a gate failure, when the model stops without changes and its reply matches a fixed phrase | `crates/blyte-agent/src/run.rs:380-389`, `660`, `1486-1506` |
| The rules given to the model forbid changing things outside the worktree. None forbids reading or copying from outside | `crates/blyte-cli/src/dispatch.rs:9-14` |
| Review flags a host command only when it names an absolute outside path and a change word, so `cp -cR ../other/.venv .venv` is not flagged | `crates/blyte-cli/src/dispatch.rs:1897-2005` |
| The Vertex token fetch runs `gcloud` with no timeout | `crates/blyte-provider/src/gemini.rs:75-78` |
| Nothing keeps the machine awake during a dispatch | no `caffeinate` in `crates/` |

Review did flag the two verified runs (host commands, outside reads); they were accepted anyway.
The flag did not say what was at stake.

## Decisions

1. **Base check, on by default.** Before the first model call, a dispatch with a gate runs the
   gate once on the untouched worktree, after `setup`. If any step fails, the run ends as
   `blocked` with zero turns, naming the failing steps and the end of their output. No model
   call is made. This is the clear error the report asks for, and it needs no configuration.
   - Cost: one extra gate run per dispatch. Build caches it warms are reused by the final gate.
   - A task whose purpose is to fix a gate that already fails passes `--no-base-check`
     (or `base_check = false` under `[dispatch]`). The `blocked` message says so.
   - Research runs have no such gate and are unchanged.
2. **A rule against borrowed environments.** The model is told not to copy, link or use a
   dependency environment (`.venv`, `node_modules`, a package cache entry) from outside the
   worktree, and to stop and say so if the environment cannot be built.
3. **A review flag that says what is at stake.** `blyte review` flags any logged command that
   copies or links an environment directory from outside the worktree, by absolute or relative
   path, including sibling worktrees and the main checkout: "the gate may have run in an
   environment from outside the worktree". It is a flag, not a refusal.
4. **Stay awake.** On macOS a dispatch holds `caffeinate -i` for the life of the run. A closed
   lid on battery still sleeps the machine; the docs say so.
5. **Bound the token fetch.** `gcloud` gets 60 seconds, then an error naming it.

## Not doing

- **Blocking reads outside the worktree in the sandbox.** Legitimate setups link the main
  checkout's environment, and package managers read their caches. Decision 1 removes the reason
  the model went looking; decision 3 catches what is left. Revisit if flagged runs keep appearing.
- **Recording which environment the gate ran in.** There is no general way to tell across
  languages; the review flag covers the case seen.
- **More model-call timeouts.** The one long wait was machine sleep.

## Check before merging

Dispatch a trivial task in the affected repository as it stands and confirm it ends `blocked`
with zero turns and names the failing gate step.
