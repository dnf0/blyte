# ADR 0003: One owned loop, a sealed gate, and blyte as an implementer

**Status:** Accepted
**Date:** 2026-09-27
**Supersedes:** [ADR 0001](https://github.com/climate-x-org/superstate/blob/main/docs/adr/0001-progressive-staged-verification.md) (progressive staged
verification) and [ADR 0002](https://github.com/climate-x-org/superstate/blob/main/docs/adr/0002-detached-handoff-and-native-rust-harness.md) (detached handoff
and the native daemon harness)

## Context

ADRs 0001 and 0002 designed blyte (then called superstate) as an execution engine that sat around
other agents: an MCP server whose tools (`state_next`, `worktree_merge`, `blyte_handoff`, …)
drove a state machine, a worktree engine, and a detached daemon that launched headless workers
(`agy -p`, `claude -p`) milestone by milestone.

Measured in repeated paired runs, none of that paid for itself. Mechanisms that influence an agent
from outside its loop were capped or negative; the daemon and the MCP gateway were equivalent to
simpler arms at higher cost; staged verification and the state engine added turns without adding
completions. What did pay was owning the loop itself and cutting turns. The measurements are in
[Harness research](../research/2026-09-24-harness-research.md). The engine, the MCP server, the
daemon and the worktree engine were removed in September 2026.

## Decision

1. **blyte owns one loop** (`crates/blyte-agent`): it calls the model, runs its tool calls
   synchronously under deadlines, and appends the results. No hooks, wrappers or background
   polling.
2. **The gate is a command, not a tool.** `--gate` decides when a run is done; the model cannot
   call, skip or grade it. Several lines are several steps, all run, all failures reported at once.
3. **Context is folded and append-only**, with harness-written digests, so the provider's cache
   stays valid.
4. **Tools are few, fast and bounded:** `read`, `write`, `edit`, `bash` (a persistent session),
   `grep`, `glob`.
5. **Host commands are sandboxed by default** (writes, credential reads, the environment, and
   optionally the network).
6. **blyte is an implementer.** Planning, splitting work and review belong to whoever drives it,
   typically a frontier model in another harness. The integration surface is the command line, not
   a protocol server: `blyte dispatch` (one task in its own worktree, a JSON summary),
   `blyte review`, the JSON run report, and a Claude Code skill (`blyte skill install`) that
   teaches the orchestrator the workflow.
7. **Every feature earns its place in measurement**, by paired comparison on the same tasks, or it
   is removed.

## Consequences

- blyte is small enough to audit, and its behaviour is the loop you can read in `run.rs`.
- Orchestration features (planning, milestones, cross-run memory, review) are deliberately absent;
  proposals to add them are measured against doing the same in the orchestrator.
- Any harness that can run a command and read JSON can drive blyte; there is no protocol to track.
- The worktree an orchestrator needs is created by `blyte dispatch`, the one place blyte manages
  git beyond the gate's rollback.
