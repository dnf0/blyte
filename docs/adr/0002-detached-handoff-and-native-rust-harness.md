# ADR 0002: Detached Handoff & Native Rust Harness as the Singular Execution Engine

**Status:** Accepted  
**Date:** 2026-09-14  
**Deciders:** Core Engineering Team  
**Consulted:** Superstate Architecture, Antigravity Engineering, Agent Rules  

---

## 1. Context & Problem Statement

Autonomous AI coding agents operate across two fundamentally opposing cognitive regimes:

1. **Interactive Alignment & Brainstorming (High Ambiguity, Conversational):**
   The human and AI explore requirements, inspect codebases, clarify edge cases, debate architectural trade-offs, and define acceptance criteria and test verification commands.
2. **Autonomous Execution & Gating (Low Ambiguity, Deterministic):**
   Code is constructed step-by-step, subjected to strict OS-level verification tests, committed atomically, and merged back into the repository.

### The In-Loop MCP Anti-Pattern

Previously, Superstate exposed over 20 low-level state-machine tools over the Model Context Protocol (MCP): `state_init`, `state_next`, `state_prompt`, `state_apply`, `state_exec`, `worktree_create`, `worktree_merge`, `state_note`, etc.

Empirical evaluation across the 25 executable tasks of the DataCurve / TerminalBench Frontier Benchmark demonstrated that exposing in-loop state tools directly to an agent inside its active chat session is an architectural anti-pattern:

* **The Chat Trap (Context Bloat & Drift):**
  When an agent is equipped with in-loop state tools, it attempts to execute multi-step implementation plans *inside its original chat window*. It carries 40,000 to 60,000+ tokens of initial exploratory dialogue, schema dumps, and compiler logs into every single coding turn. As context length increases, reasoning accuracy degrades precipitously.
* **The Self-Supervision Paradox:**
  An LLM cannot reliably police its own verification gates. When an agent calls `state_next` in-loop, it frequently negotiates with failed assertions, rationalizes partial failures, skips verification, or hallucinates completion.
* **Tool Dithering & Latency:**
  Models burn 15–20% of their token budget and multiple round-trip turns simply deliberating over which state tool to call next (`state_show` vs `state_status` vs `state_prompt`), inflating wall-clock time and cost.
* **Empirical Benchmark Proof:**
  * **In-Loop MCP (`Condition A`):** **64.0% pass rate** ($0.25 / solve, 1,225,025 input tokens / solve).
  * **Detached Native Runtime (`Condition B`):** **76.0% pass rate** ($0.21 / solve, 1,045,102 input tokens / solve).

Condition B achieved a **+12.0 percentage point accuracy advantage** at **lower cost** and **higher token efficiency** by enforcing zero-turn prompt injection and out-of-band deterministic gating.

---

## 2. Decision: Strict Handoff Gateway & Singular Native Rust Harness

We will deprecate and remove all in-loop state mutation tools from the public MCP server manifest, establishing the **Detached Handoff** and the **Native Rust Daemon Harness** as the singular, authoritative execution architecture for Superstate.

```
┌────────────────────────────────────────────────────────────────────────┐
│ 1. INTERACTIVE CHAT (Google Antigravity / Claude Code / Cursor)        │
│    Role: Conversational Alignment & Milestone Specification ONLY       │
│                                                                        │
│    MCP Gateway Tool Surface:                                           │
│    • `superstate_handoff` (Initiates detached execution; always true) │
│    • `superstate_status`  (Interrogate active background jobs)        │
│    • `superstate_cancel`  (Gracefully abort a running job)            │
│    • `superstate_doctor`  (Environment & driver preflight check)       │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    │ Single Tool Call:
                                    │ `superstate_handoff(goal, milestones, verify_cmd, ...)`
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 2. SUPERSTATE DETACHED GATEWAY (`crates/superstate-mcp/src/server.rs`)  │
│                                                                        │
│  1. Runs `execute_preflight` on verification commands.                 │
│  2. Captures target branch and provisions isolated worktree `.wt/<job>`.│
│  3. Writes atomic `.agent-state/state.json`.                           │
│  4. Spawns detached daemon: `superstate state daemon --state ...`      │
│  5. Releases chat session immediately:                                 │
│     "Execution detached to job-9a1b in .wt/job-9a1b. Chat is free."    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                      Chat Closes   │ Detached Process Runs
                      Human is Free │ Headlessly in Background
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 3. NATIVE RUST HARNESS (`crates/superstate-core/src/daemon.rs`)        │
│                                                                        │
│  Loop per milestone:                                                   │
│   a. Render turnkey bounded prompt (0 conversational history).         │
│   b. Launch headless driver worker (`agy -p` / `claude -p`).           │
│   c. Execute OS verification gate (`--verify-cmd` + git clean check).  │
│   d. Gate Pass: Create atomic checkpoint commit, advance milestone.    │
│   e. Gate Fail: Feed stderr diff back to worker (up to max retries).   │
│   f. Complete: Merge `.wt/<job>` to base branch, write completion JSON.│
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 4. AUDIT & NOTIFICATION                                                │
│    • Real-time interrogation: `superstate state status`                │
│    • Completion artifact: `.agent-state/completion.json`               │
│    • OS terminal bell / system notification ping                       │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Specifications

### A. Pruning the Public MCP Tool Manifest
The `superstate-mcp` server (`crates/superstate-mcp/src/server.rs`) will expose **only 4 tools** to client agents:

| Tool Name | Parameters | Purpose |
|---|---|---|
| **`superstate_handoff`** | `goal`, `milestones`, `verify_cmd`, `driver`, `files` | The sole execution entrypoint. Validates verify commands, provisions isolated worktrees, and launches the native background daemon. Always detaches (`detach: true` enforced). |
| **`superstate_status`** | `state_path` (optional) | Interrogates active background daemon state, current milestone, retry count, and blocker diagnostics. |
| **`superstate_cancel`** | `slug` / `state_path` | Signals and gracefully terminates an active background daemon process via SIGTERM/SIGKILL. |
| **`superstate_doctor`** | `driver` (optional) | Runs the 5-point fail-closed preflight probe to verify binaries, git baseline, and headless driver availability. |

All internal mutation tools (`state_init`, `state_next`, `state_prompt`, `state_apply`, `worktree_create`, `worktree_merge`, etc.) are removed from the MCP `list_tools` handler. They remain accessible strictly via the internal Rust CLI for manual scripting or debugging.

### B. The Native Rust Daemon Runner (`DaemonRunner`)
The `DaemonRunner` in `crates/superstate-core/src/daemon.rs` serves as the authoritative Condition B execution harness:
1. **Worktree Isolation:** By default, all execution occurs in an isolated git worktree (`.wt/<slug>`), keeping the user's primary working tree 100% clean.
2. **Headless Driver Expansion:** Presets for `agy` (`-p --dangerously-skip-permissions`) and `claude` (`-p --dangerously-skip-permissions` / `--permission-mode bypassPermissions`) ensure unattended non-interactive execution without blocking on confirmation prompts.
3. **Turnkey Bounded Context:** Prompts contain only the active milestone task, relevant files, dependencies, and epistemic ledger notes. Each turn consumes < 5,000 tokens of input, eliminating the Chat Trap.
4. **Deterministic OS Gating:** Gates are executed by the host operating system, not the LLM. If tests fail, the stderr output is injected into the next retry attempt via `format_retry_failure_section`.
5. **Lifecycle Finalization:** Upon completion of the final milestone:
   - Merges the worktree branch cleanly back into the base feature branch.
   - Cleans up the ephemeral `.wt/<slug>` directory.
   - Writes `.agent-state/completion.json` containing diff statistics, milestone timings, and verification proof.

---

## 4. Consequences & Impact

### Positive
* **Elimination of the Chat Trap:** Interactive chat context is discarded upon handoff. Background workers operate with pristine, bounded context windows, eliminating context degradation.
* **Higher Task Success:** Standardizing on the 76.0% Condition B architecture ensures all Superstate runs benefit from the higher pass rate observed in empirical benchmarks.
* **Zero Cognitive Overhead:** Developers and agents do not need to choose among 20+ state tools. The prompt workflow is simply: *Align on plan $\rightarrow$ Call `superstate_handoff` $\rightarrow$ Chat detaches*.
* **Workspace Safety:** Development takes place exclusively inside isolated worktrees, preventing accidental corruption of active developer workspaces.

### Negative / Trade-Offs
* **Loss of In-Loop Micro-Stepping in Chat:** Developers who previously watched intermediate state transitions in their conversational chat stream will now monitor progress via `superstate state status`, logs, or completion notifications.
* **Deprecation of Legacy MCP Tooling:** External scripts or legacy skills that called `mcp_superstate_state_next` directly will need to migrate to `superstate_handoff` or invoke the local CLI (`superstate state next`).

---

## 5. References
* [Definitive Benchmark Report: Superstate Native Runtime vs. Superpowers & Vanilla](file:///Users/danielfisher/.gemini/antigravity-cli/brain/40d9a6a9-9f83-47fb-8967-d564a56e3b2f/frontier-4arm-definitive-benchmark-report.md)
* [Superstate Detached Handoff Protocol & Execution Engine Spec](file:///Users/danielfisher/.gemini/antigravity-cli/brain/40d9a6a9-9f83-47fb-8967-d564a56e3b2f/superstate-detached-handoff-spec.md)
* [ADR 0001: Progressive Staged Verification & Regression Quarantine](file:///Users/danielfisher/repos/superstate/docs/adr/0001-progressive-staged-verification.md)
