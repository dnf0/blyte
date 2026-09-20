# The Superstate Manifesto
## Invariants for Autonomous Software Engineering

---

### The Fallacy of Chat-Driven Development

Software engineering is not conversation. It is a stateful, deterministic systems discipline.

The prevailing paradigm of AI coding treats software engineering as an extended conversational chat session: an LLM receives a prompt, predicts tokens, issues unverified edits into a workspace, and declares itself "done."

On toy puzzles and single-file scripts, this illusion holds. On real software engineering repositories, it collapses:

1. **The Confidence Trap (Done While Wrong):** An LLM has no mechanical friction. Its internal feeling of confidence is completely uncorrelated with code correctness. In empirical software engineering evaluations, an agent left to judge its own completion quits prematurely and incorrectly in over 80% of its losses—spending a fraction of its compute budget while abandoning broken code.
2. **Cascading Edit Pollution:** When a conversational agent fails a test or breaks a build, it does not roll back. It apologizes and makes further edits on top of broken edits. Within three turns, the repository is littered with contradictory partial fixes, dangling syntax, and wrecked git trees.
3. **Context Collapse:** Production software repositories are loud. Multi-thousand line compiler dumps, wall-of-text test logs, and runaway polling loops saturate the context window, triggering catastrophic forgetting, amnesia, and prompt truncation.

Prompt engineering cannot fix these failures, because the failure is architectural. You cannot instruct an untrusted worker to supervise itself.

---

### The Core Axiom

> **Autonomous software engineering requires a strict decoupling between the Worker Plane and the Control Plane.**
>
> The model is an untrusted, interchangeable reasoning engine.  
> The control plane is a deterministic, unyielding supervisor.  
>
> **The worker proposes changes. The environment decides truth.**

---

### The Five Invariants of Autonomous Execution

#### I. State Belongs to the Environment, Not the Token Buffer
The context window of a language model is volatile, lossy, and ephemeral. It is subject to compaction, prompt truncation, and attention degradation. Ground truth must never reside in conversational memory.

Ground truth resides exclusively in the repository:
- The filesystem, the git commit tree, the dependency graph, and an externalized state machine.
- Every milestone, state transition, checkpoint commit, and environmental discovery is externalized to structured state on disk (`.agent-state/state.json`).
- The model remains stateless. The repository and the control plane maintain the state.

#### II. Mechanical Verification Over Self-Assessment
An LLM must never be allowed to grade its own homework or declare itself finished.

"Done" is not a qualitative judgement, a polite conversational sign-off, or an LLM reflection. "Done" is a binary reality certified by deterministic verification:
- A milestone is complete if and only if its verification command exits `0`.
- An ungated task or a vacuous check (`exit 0`, `true`) verifies nothing; it is a concession.
- If a verification command fails, the worker cannot proceed, cannot commit, and cannot exit.

#### III. Scoped Git Rollbacks Over Cascading Edit Pollution
In software engineering, a bad edit is worse than no edit.

When an attempt fails:
- Unverified, failing code must never accumulate in the working tree across turns.
- Every milestone boundary establishes an immutable git checkpoint.
- If an execution turn fails its verification gate, the control plane executes a scoped, deterministic hard rollback (`git reset --hard <checkpoint>`) before the agent is allowed to retry.
- The agent begins every new turn from a clean, verified state—armed with the exact failure telemetry of the previous attempt, not a corrupted workspace.

#### IV. Mechanical Perimeter Guards & AST Hygiene
You cannot regulate an agent's context through system prompt guidelines alone. Under task pressure, an agent will take the shortest path to an answer, regardless of written rules.

Context boundaries must be mechanically enforced at the execution perimeter:
- **PreToolUse Lifecycle Interception:** Runaway busy-wait polling loops must be blocked with mandatory cooldown debouncing at the system call boundary.
- **Deterministic AST Slicing:** Massive file reads must be intercepted and served with precise, Tree-sitter AST symbol outlines and line numbers—zero hallucinations, zero latency, and zero token waste.
- **Tool Output Shunting:** Multi-thousand-line compiler dumps and noisy test logs must be mechanically distilled into head, tail, and diagnostic signals before reaching the prompt.

#### V. Autonomous TDD as the Problem-Solving Engine
Autonomous software engineering cannot rely on synthetic hand-authored scaffolds or answer-key leakage. Pre-seeding maintainer reproduction tests is cheating; it turns an open-ended engineering diagnosis into a narrow test-passing game.

The canonical execution engine for autonomous engineering is mechanical Test-Driven Development:
1. **Red (Milestone 1 — Reproduction):** The agent diagnoses the issue and authors an isolated reproduction test. The verification gate mechanically asserts that the test compiles and **fails** against the pristine repository. A checkpoint commit is frozen.
2. **Green (Milestone 2 — Implementation):** The agent implements the minimal fix. The gate mechanically asserts that the reproduction test **and** the full repository test suite pass (exit `0`).
3. **Commit & Handoff:** The state machine records completion, creates an atomic commit, and yields verified code.

---

### The Architecture of Separation

Autonomous execution requires an uncompromising division of responsibilities:

```
┌────────────────────────────────────────────────────────┐
│                   THE CONTROL PLANE                    │
│      (Superstate: Compiled Rust Daemon & Engine)       │
│                                                        │
│  - Topological Milestone DAG & State Machine           │
│  - Mechanical Verification Gates (Exit Code Evaluator) │
│  - Scoped Git Checkpoint & Hard Rollback Engine        │
│  - Deterministic AST Impact Slicing (Tree-Sitter)      │
│  - PreToolUse Lifecycle Guards (Debounce & Shunt)      │
└───────────────────────────┬────────────────────────────┘
                            │ Dispatches bounded turnkey prompts;
                            │ Enforces verification & rollback
                            ▼
┌────────────────────────────────────────────────────────┐
│                    THE WORKER PLANE                    │
│        (Interchangeable Agent / Model Runtimes)        │
│                                                        │
│  - Antigravity / Claude Code / OpenAI / Local LLMs     │
│  - Proposes diffs, reads slices, executes tool calls   │
│  - Subject to timeout, budget governor, and lease      │
└────────────────────────────────────────────────────────┘
```

The **Worker Plane** is untrusted. It can be any model, any provider, or any CLI driver. It does not hold state, it does not decide completion, and it cannot bypass the gate.

The **Control Plane** is unyielding. It treats the worker as a transient compute process, bounding its context, validating its claims against compilers and test runners, and rolling back its mistakes until mechanical proof is achieved.

---

### The Standard for Autonomous Engineering

We reject the notion that AI coding should remain an interactive chat assistant that needs continuous human babysitting to catch its mistakes and premature declarations of victory.

Real software engineering is:
- **Detached:** Dispatched into the background as an autonomous daemon that respects time leases and budgets.
- **Git-Native:** Operating directly on commits, worktrees, branches, and diffs.
- **Measurable:** Judged by held-out test suites, rigorous gate precision, and zero-leakage benchmarks.
- **Deterministic:** Guided by AST syntax graphs and shell exit codes, not prompt vibes.

---

### In Summary

> **We value:**
> - **Mechanical verification** over agent self-confidence.
> - **Scoped git rollbacks** over conversational apologies.
> - **Deterministic AST slicing** over token-bloated context dumps.
> - **Persistent environmental state** over ephemeral chat histories.
> - **Autonomous Red/Green TDD** over synthetic prompt scaffolds.

**This is Superstate.**
