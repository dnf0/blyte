# Blyte MCP (`blyte-mcp`)

The Model Context Protocol (MCP) server for Blyte.

## Architectural Model: Strict Detached Gateway (ADR 0002)

Per **[ADR 0002](../../docs/adr/0002-detached-handoff-and-native-rust-harness.md)**, `blyte-mcp` is designed strictly as a **Detached Handoff Gateway**. 

Exposing granular state mutation tools (`state_next`, `state_prompt`, `worktree_*`) over MCP inside an interactive LLM chat session is an anti-pattern (The Chat Trap). It balloons context size, invites self-supervision hallucinations, and forces agents to babysit execution loops.

Instead, `blyte-mcp` exposes strictly **4 Gateway tools** designed to validate intent, provision isolated git workspaces, and cleanly hand off execution to the native Rust daemon harness (`DaemonRunner`):

```
┌─────────────────────────────────┐
│   Chat Session (User + LLM)     │
│   (Claude Code / Cursor / Agy)  │
└────────────────┬────────────────┘
                 │
                 │ 1. blyte_handoff(goal, milestones, driver)
                 ▼
┌─────────────────────────────────┐
│       blyte-mcp            │
│  (Strict Detached Gateway)      │
└────────────────┬────────────────┘
                 │
                 │ 2. Preflight verification command
                 │ 3. Provision .wt/<slug> worktree
                 │ 4. Spawn detached daemon process
                 ▼
┌─────────────────────────────────┐
│   Native Rust Daemon Harness    │ ◄─── Driven by OS / Rust
│  (DaemonRunner in blyte)   │      Zero Context Drift
└────────────────┬────────────────┘      Auto-Merge on Pass
                 │
                 │ Headless Driver Loop (Claude / Agy / Shell)
                 ▼
┌─────────────────────────────────┐
│   Automated Worktree Merge &    │
│  .agent-state/completion.json   │
└─────────────────────────────────┘
```

## Available Tools

The MCP server registers strictly 4 tools via `tools/list`:

### 1. `blyte_handoff`
Initiates autonomous, detached task execution in an isolated git worktree.
- `goal` (*optional*): Single-task goal description.
- `verify_cmd` (*optional*): Verification command for single-task goal.
- `milestones` (*optional*): Array of milestone objects `[{"title": "...", "verify_cmd": "...", "files": [...]}]`.
- `driver` (*optional*): Headless driver to execute work (`"claude"`, `"agy"`, or custom shell command).
- `worktree` (*boolean, default: true*): Run in an isolated git worktree (`.wt/<slug>`).
- `detach` (*boolean, default: true*): Launch the native daemon in the background and return immediately.

### 2. `blyte_status`
Query the real-time execution status, active milestone, milestone DAG progress, and daemon health without mutating state.
- `json` (*boolean, default: false*): Return machine-readable structured JSON.
- `state_path` (*optional*): Path to specific state file (defaults to auto-resolving branch-scoped state).

### 3. `blyte_cancel`
Gracefully terminate running background daemons and clean up PID records.
- `slug` (*optional*): Cancel a specific job daemon (defaults to active branch/default job).
- `all` (*boolean, default: false*): Cancel all active daemons across the entire repository.

### 4. `blyte_doctor`
Fail-closed diagnostic probe validating binary integrity, driver availability, MCP loopback, and workspace health.
- `fix` (*boolean, default: false*): Automatically repair benign issues (stale state, broken worktree references).

## Legacy Tool Deprecation

Calling any legacy in-loop tool (`state_init`, `state_next`, `state_prompt`, `state_apply`, `worktree_create`, etc.) returns a descriptive JSON-RPC error:
> *"The tool '<name>' has been deprecated. In-loop MCP execution is an anti-pattern. Use 'blyte_handoff' to initiate autonomous detached execution in an isolated worktree."*
