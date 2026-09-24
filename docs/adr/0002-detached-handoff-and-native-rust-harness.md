# ADR 0002: Detached Handoff & Native Rust Harness as the Singular Execution Engine

**Status:** Accepted  
**Date:** 2026-09-14  
**Deciders:** Core Engineering Team  
**Consulted:** Blyte Architecture, Antigravity Engineering, Agent Rules  

---

## 1. Context & Problem Statement

Autonomous AI coding agents operate across two fundamentally opposing cognitive regimes:

1. **Interactive Alignment & Brainstorming (High Ambiguity, Conversational):**
   The human and AI explore requirements, inspect codebases, clarify edge cases, debate architectural trade-offs, and define acceptance criteria and test verification commands.
2. **Autonomous Execution & Gating (Low Ambiguity, Deterministic):**
   Code is constructed step-by-step, subjected to strict OS-level verification tests, committed atomically, and merged back into the repository.

### The In-Loop MCP Anti-Pattern

Previously, Blyte exposed over 20 low-level state-machine tools over the Model Context Protocol (MCP): `state_init`, `state_next`, `state_prompt`, `state_apply`, `state_exec`, `worktree_create`, `worktree_merge`, `state_note`, etc.

Empirical evaluation conducted on 2026-09-13 across the DataCurve / TerminalBench Frontier Benchmark (30 evaluation tasks running on driver `agy` / Gemini 3.8 Flash High) compared an in-loop MCP state-machine workflow against a prototype out-of-loop detached CLI harness:

* **The Chat Trap (Context Bloat & Drift):**
  When an agent is equipped with in-loop state tools, it attempts to execute multi-step implementation plans *inside its original chat window*. It carries 40,000 to 60,000+ tokens of initial exploratory dialogue, schema dumps, and compiler logs into every single coding turn. As context length increases, reasoning accuracy degrades precipitously.
* **The Self-Supervision Paradox:**
  An LLM cannot reliably police its own verification gates. When an agent calls `state_next` in-loop, it frequently negotiates with failed assertions, rationalizes partial failures, skips verification, or hallucinates completion.
* **Tool Dithering & Latency:**
  Models burn 15–20% of their token budget and multiple round-trip turns simply deliberating over which state tool to call next (`state_show` vs `state_status` vs `state_prompt`), inflating wall-clock time and cost.
* **Empirical Benchmark Evidence (superseded -- see Addendum, 2026-09-15):**
  * *Executable Subset (25 tasks):* Excluding 5 tasks that failed uniformly across all arms due to upstream Docker Hub 404 image resolution errors, the prototype detached CLI harness (`Condition B`) achieved **76.0% pass rate (19/25)** at **$0.21 / solve**, compared to **64.0% (16/25)** at **$0.25 / solve** for in-loop MCP (`Condition A`).
  * *Full Suite (30 tasks):* **63.3% (19/30)** vs **53.3% (16/30)**.
  * These figures remain accurate as recorded, but the 2026-09-15 addendum below establishes that they were measured against baselines that received no prompt support while the Blyte arms did, and that the comparison against `vanilla` is the more informative one.

---

## 2. Decision: Strict Handoff Gateway & Singular Native Rust Harness

We will deprecate and remove all in-loop state mutation tools from the public MCP server manifest, establishing the **Detached Handoff** and the **Native Rust Daemon Harness** as the singular, authoritative execution architecture for Blyte.

```
┌────────────────────────────────────────────────────────────────────────┐
│ 1. INTERACTIVE CHAT (Google Antigravity / Claude Code / Cursor)        │
│    Role: Conversational Alignment & Milestone Specification ONLY       │
│                                                                        │
│    MCP Gateway Tool Surface:                                           │
│    • `blyte_handoff` (Initiates detached execution; always true) │
│    • `blyte_status`  (Interrogate active background jobs)        │
│    • `blyte_cancel`  (Gracefully abort a running job)            │
│    • `blyte_doctor`  (Environment & driver preflight check)       │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    │ Single Tool Call:
                                    │ `blyte_handoff(goal, milestones, verify_cmd, ...)`
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 2. BLYTE DETACHED GATEWAY (`crates/blyte-mcp/src/server.rs`)  │
│                                                                        │
│  1. Runs `execute_preflight` on verification commands.                 │
│  2. Captures target branch and provisions isolated worktree `.wt/<job>`.│
│  3. Writes atomic `.agent-state/state.json`.                           │
│  4. Spawns detached daemon: `blyte state daemon --state ...`      │
│  5. Releases chat session immediately:                                 │
│     "Execution detached to job-9a1b in .wt/job-9a1b. Chat is free."    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                      Chat Closes   │ Detached Process Runs
                      Human is Free │ Headlessly in Background
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 3. NATIVE RUST HARNESS (`crates/blyte-core/src/daemon.rs`)        │
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
│    • Real-time interrogation: `blyte state status`                │
│    • Completion artifact: `.agent-state/completion.json`               │
│    • OS terminal bell / system notification ping                       │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Specifications

### A. Pruning the Public MCP Tool Manifest
The `blyte-mcp` server (`crates/blyte-mcp/src/server.rs`) will expose **only 4 tools** to client agents:

| Tool Name | Parameters | Purpose |
|---|---|---|
| **`blyte_handoff`** | `goal`, `milestones`, `verify_cmd`, `driver`, `files` | The sole execution entrypoint. Validates verify commands, provisions isolated worktrees, and launches the native background daemon. Always detaches (`detach: true` enforced). |
| **`blyte_status`** | `state_path` (optional) | Interrogates active background daemon state, current milestone, retry count, and blocker diagnostics. |
| **`blyte_cancel`** | `slug` / `state_path` | Signals and gracefully terminates an active background daemon process via SIGTERM/SIGKILL. |
| **`blyte_doctor`** | `driver` (optional) | Runs the 5-point fail-closed preflight probe to verify binaries, git baseline, and headless driver availability. |

All internal mutation tools (`state_init`, `state_next`, `state_prompt`, `state_apply`, `worktree_create`, `worktree_merge`, etc.) are removed from the MCP `list_tools` handler. They remain accessible strictly via the internal Rust CLI for manual scripting or debugging.

### B. The Native Rust Daemon Runner (`DaemonRunner`)
The `DaemonRunner` in `crates/blyte-core/src/daemon.rs` serves as the authoritative Condition B execution harness:
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
* **Higher Task Success:** Standardizing on the 76.0% Condition B architecture ensures all Blyte runs benefit from the higher pass rate observed in empirical benchmarks.
* **Zero Cognitive Overhead:** Developers and agents do not need to choose among 20+ state tools. The prompt workflow is simply: *Align on plan $\rightarrow$ Call `blyte_handoff` $\rightarrow$ Chat detaches*.
* **Workspace Safety:** Development takes place exclusively inside isolated worktrees, preventing accidental corruption of active developer workspaces.

### Negative / Trade-Offs
* **Loss of In-Loop Micro-Stepping in Chat:** Developers who previously watched intermediate state transitions in their conversational chat stream will now monitor progress via `blyte state status`, logs, or completion notifications.
* **Deprecation of Legacy MCP Tooling:** External scripts or legacy skills that called `mcp_blyte_state_next` directly will need to migrate to `blyte_handoff` or invoke the local CLI (`blyte state next`).

---

## 5. References
* Internal Benchmark Report: *"Frontier 4-Arm Definitive Benchmark Report"* (DataCurve/TerminalBench 30-task evaluation, 2026-09-13).
* Internal Design Spec: *"Blyte Detached Handoff Protocol & Execution Engine Specification"* (2026-09-13).
* [ADR 0001: Progressive Staged Verification & Regression Quarantine](0001-progressive-staged-verification.md)

---

## Addendum: What Actually Drives the Result (2026-09-15)

The original evidence compared `Condition B` against `Condition A` -- two Blyte
variants. It did not isolate what produced the advantage. A controlled 2x2 was run on
2026-09-15 to answer that, because a confound had been identified: arm-specific prompt
contracts (`TARGETED_EXECUTION_CONTRACT`, 10 behavioural rules) were injected into the
Blyte arms and into no baseline. `vanilla` has no prompt construction at all. So the
published lift measured *control plane + hand-tuned prompt* against *neither*.

### Method

20 TerminalBench tasks, driver `agy` / Gemini 3.8 Flash High, `blyte_mcp` (the arm
that produced the 76%) against `vanilla`, each run with the prompt contracts disabled and
enabled. 80 runs, no infrastructure failures. `sqlite-db-truncate` was excluded: it ships
no verifier, so its grader exits 127 for every arm regardless of the work done.

### Result

| Arm | contracts OFF | contracts ON | matched delta |
|---|---|---|---|
| `vanilla` | 9/20 (45%) | 13/20 (65%) | **+4 tasks** |
| `blyte_mcp` | **14/20 (70%)** | 14/20 (70%) | **0 tasks** |

### Findings

1. **The control plane owns the effect.** With no prompt engineering of any kind,
   `blyte_mcp` solves 70% against `vanilla`'s 45% -- a 25 percentage point lift. This
   is the claim the architecture should rest on, and it is cleaner than the original
   Condition A/B comparison because the treatment is the control plane alone.

2. **The prompt contracts contribute nothing to Blyte.** Zero change across 20
   matched tasks. They are inert where they are currently applied.

3. **They are a partial substitute for the control plane, not a complement.** They lift
   `vanilla` by 4 tasks, toward but not to Blyte's level. Prompt discipline supplies
   some of what the state machine supplies; the state machine supplies more, and does so
   without prompt tuning.

4. **They are unsafe off short tasks.** On the 9 DataCurve repo tasks (~30 min each, where
   instructions explicitly direct the agent to "explore the codebase first"), the same
   rules took `vanilla` from 4/9 to 1/9. Several rules -- "zero global exploration", "never
   cat entire files" -- are a bet that the work has already been located. On a short
   self-contained task that bet pays; on a repository-scale task it is fatal. The control
   plane's advantage held on both families; the prompt's reversed sign.

### Mechanism

Across the 30-task run, wall clock tracks solve rate almost monotonically
(vanilla 10.4 min -> 13 solves; superpowers 10.2 -> 14; Condition A 13.3 -> 16;
Condition B 18.6 -> 19), while turn counts are flat across all four arms (0.83-1.00).
Budget is not the constraint: `vanilla` was granted 43 minutes and used 10.4, stopping
voluntarily with three quarters of its allowance unspent, and every `vanilla` failure
recorded `turns=1`.

The candidate mechanism is that the state machine denies the agent an easy exit. An
unassisted agent decides for itself when it is done and is frequently wrong; Blyte
defines "done" externally, against a named verification command, so the agent keeps working
past the point at which it would have self-terminated. Among runs where real work occurred
(cost >= $0.05), Condition B converted 69% against `vanilla`'s 47%, so it is not only
persistence -- the additional work also lands better.

This mechanism is consistent with the data but not yet established causally.

### Consequences

* The arm-specific prompt contracts should be removed. They add nothing to Blyte, they
  are harmful outside it, and removing them makes the headline claim strictly cleaner.
* The claim to make is **"external state and verification produce a substantial solve-rate
  lift over an unassisted agent, with no prompt engineering"**, not a single headline
  percentage. The 76% figure is a filtered 19/25 and should not be quoted alone.
* Statistical caution: n=20 per cell puts the headline gap at p~0.10 in isolation. Its
  weight comes from being the third independent run in the same direction (19/30, 16/30,
  14/20), not from any single sample.


---

## Addendum 2: The Daemon Does Not Replace `blyte_mcp` (2026-09-16) — SUPERSEDED, see Addendum 3

This ADR deprecates the in-loop state tools in favour of the detached daemon. The evidence
now says that swaps out the best-performing arm for a worse one.

### The distinction the original decision missed

The critique in section 1 -- the chat trap, the self-supervision paradox, tool dithering --
is about **an agent driving its own state loop**. That is `blyte_agent_mcp`
(Condition A, 64%), and the reasoning holds: it should be deprecated.

`blyte_mcp` (Condition B, the arm that produced the 76%) is a different thing. The
**harness** drives the loop; the agent only takes turns. It never had the pathology the ADR
describes. Removing the `state_*` tools wholesale took away the capability the winning arm
depends on, in order to fix a problem that arm did not have.

The axis that matters is **who drives the loop**, not which transport is used.

### Measurements

Driver `agy` / Gemini 3.8 Flash High throughout, prompt contracts off, no infrastructure
failures.

| Comparison | `blyte_mcp` | `blyte_daemon` |
|---|---|---|
| 20 TerminalBench tasks, 2 repeats (40 runs/arm) | **67.5%** | 60.0% |
| 3 tasks with real verification commands, 5 repeats | **15/15** | 10/15 |
| cost per run | **$0.20** | $0.42 |

Three independent measurements, all in the same direction: the daemon is equal or worse at
roughly double the cost. `blyte_mcp` is also the only arm that beats `vanilla`
significantly (67.5% vs 45.0%, p=0.037) while matching its cost and running faster.

The daemon's deficit is concentrated rather than uniform -- it matches `blyte_mcp` on
most tasks and fails badly on a few. It is gate-driven, so it must satisfy its verification
command to complete, whereas `blyte_mcp` checks the gate after the agent concludes and
moves on within a turn budget. That makes the daemon far more sensitive to the quality of
the verification command it is handed.

### Decision

1. **Do not deprecate `blyte_mcp`.** Restore the `state_*` tools for orchestrator use.
   Gate them by **caller**: available to a harness or orchestrator driving the loop, closed
   to an agent running them inside its own chat session.
2. **Deprecate `blyte_agent_mcp`.** The ADR's original reasoning applies here and the
   benchmark supports it (64% against Condition B's 76%).
3. **Keep the daemon as a deployment mode**, not the sole execution path. It is the right
   shape for genuine fire-and-forget handoff, and its cost premium may be acceptable when
   nobody is waiting. It should not be the only way to run Blyte.

### What is not established

* Why the daemon fails the small number of tasks it fails. It is not the stopping rule: a
  run with advisory gating enabled never triggered the advisory branch, and the tasks that
  passed did so by satisfying their verification command after several retries. The 0/5 that
  prompted that investigation was run-to-run variance, not a mechanism.
* Whether the daemon's cost premium narrows with better verification commands. It persisted
  across both gated and ungated conditions, but the gated sample is only three tasks.


---

## Addendum 3: Addendum 2 Was Measuring a Harness Bug (2026-09-16)

Addendum 2 concluded that `blyte_daemon` is equal or worse than `blyte_mcp` at
roughly double the cost. **That conclusion is withdrawn.** The deficit it measured was a
defect in the evaluation harness, not in the architecture.

### The bug

`agy` truncates any turn longer than its own `--print-timeout`, which defaults to 5 minutes.
`AgyCliDriver` sets it from the granted budget for every arm it drives, with a source comment
stating the consequence plainly: *"every turn longer than that is truncated by the CLI
regardless of the budget the harness granted, and the arm is scored as having failed."*

The daemon spawns its own worker through a wrapper and never set it. Every daemon turn was
capped at 5 minutes no matter what budget the task was granted.

### What that did to the numbers

The failure was entirely length-dependent, which is why it looked like a task-specific
weakness rather than a harness fault:

| Task | Typical length | Before | After |
|---|---|---|---|
| `openssl-selfsigned-cert` | ~2 min | 5/5 | 5/5 |
| `db-wal-recovery` | ~2.5 min | 5/5 | 5/5 |
| `build-cython-ext` | 30-45 min | **0/5** | **5/5** |

With the fix the daemon scores **15/15** on the gated set against `blyte_mcp`'s 15/15,
at **$0.34/run against $0.48**. It also completes `build-cython-ext` in 11.6 minutes rather
than the 32-42 it previously spent re-attempting work that kept being truncated.

### Consequences

* Addendum 2's cost and accuracy comparison is void. So is the 60% figure for the daemon in
  the 20-task run, and every other daemon measurement taken before this fix.
* The daemon needs re-measuring across the full task set before any claim is made about it.
* **The rest of Addendum 2 stands**, because it never depended on the daemon's scores: the
  ADR's critique applies to an agent driving its own state loop (`blyte_agent_mcp`,
  64%), not to `blyte_mcp`, which is harness-driven and scored 76%. Removing the
  `state_*` tools wholesale still took away the winning arm's capability to fix a problem it
  did not have, and they should be gated by caller rather than deleted.

### Note on method

This was the fourth thing the daemon's wrapper dropped relative to `AgyCliDriver`, after
model pinning, usage telemetry and the container execution guidance. Each produced a
plausible-looking architectural finding that dissolved on inspection. Any arm that bypasses
the shared driver needs its invocation diffed against that driver before its numbers are
trusted -- the failures are silent and they all look like weaknesses of the thing under test.

---

## Addendum 4: Daemon and `blyte_mcp` Are Equivalent (2026-09-16)

Addendum 3 withdrew the daemon's measured deficit and said it needed re-measuring before any
claim. That is now done, on 20 TerminalBench tasks each carrying a hand-authored verification
command, with both arms verified to receive identical inputs before the numbers were read.

| Arm | Solved | Mean wall | Cost/run | Total |
|---|---|---|---|---|
| `blyte_daemon` | **12/20 (60%)** | 6.3 min | **$0.25** | $4.99 |
| `blyte_mcp` | **12/20 (60%)** | 6.2 min | $0.33 | $6.51 |

Across 20 paired tasks the two arms **disagree on none**. Not similar totals -- the same
tasks solved and the same tasks failed, with the daemon 23% cheaper overall.

### Every earlier daemon deficit was harness, not architecture

| Previously reported | Actual cause |
|---|---|
| 2x cost premium | the worker wrapper never set `--print-timeout`, so agy truncated every turn at its 5-minute default |
| losing 5-0 on gated tasks | `blyte_mcp` was missed when fixtures were wired in, so it ran on `true` while the daemon ran on a strict gate |
| 60% vs 67.5% on 20 tasks | both of the above |

All three were differences in what the arms received, not in how they execute. The daemon is
the recurring offender because it spawns its own worker and so bypasses `AgyCliDriver`, which
performs that setup for every other arm. `tests/test_arm_input_parity.py` in superstate-eval
now fails when the arms diverge.

### Decision

* **The detached daemon is a legitimate default.** It matches the harness-driven arm exactly
  on accuracy, costs less, and additionally supports genuine fire-and-forget handoff.
* **The reasoning in this ADR still needs correcting.** The case for the daemon is not that
  in-loop state tools are an anti-pattern in general -- it is that the daemon is equivalent
  and cheaper. The anti-pattern critique applies specifically to `blyte_agent_mcp`
  (Condition A, 64%), where the *agent* drives its own state loop.
* **`blyte_mcp` should not have been removed.** It is harness-driven, it produced the
  76% result, and an orchestrator needs the `state_*` tools it depends on. Gate those tools
  by caller rather than deleting them: available to an orchestrator, closed to an agent
  running them inside its own chat session.

### Caveats

* One repeat per task. The arms agreeing on all 20 is strong, but the absolute 60% is a
  single sample.
* `vanilla` was not in this run. The control-plane lift over an unassisted agent rests on the
  earlier 40-runs-per-arm measurement (67.5% vs 45.0%, p=0.037).
