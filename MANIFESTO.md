# The Superstate Manifesto
## Invariants for Autonomous Software Engineering

---

### The Fallacy of Chat-Driven Development

Software engineering is not conversation. It is a stateful, deterministic systems discipline.

The prevailing paradigm of AI coding treats software engineering as an extended conversational chat session: an LLM receives a prompt, predicts tokens, issues unverified edits into a workspace, and declares itself "done."

On toy puzzles and single-file scripts, this illusion holds. On real software engineering repositories, it collapses:

1. **The Confidence Trap (Done While Wrong):** An LLM has no mechanical friction. Its internal feeling of confidence is completely uncorrelated with code correctness. On terminal-bench, an unmanaged agent's dominant failure mode is declaring done while wrong — **15 of 18 losses**, at **$0.069 median spend against the $0.187 it spends on tasks it solves**. It quits early and confidently. [FINDINGS 2]

   **This is family-specific, and it inverts.** On the DataCurve set the same unmanaged judgement scored **43% against superstate's 23–27%**: where the gate is weak, replacing the agent's judgement with it makes things worse, not better. A control plane is a gain only when the gate is the better judge. [FINDINGS 7]
2. **Cascading Edit Pollution:** When a conversational agent fails a test or breaks a build, it does not roll back. It apologizes and makes further edits on top of broken edits. Measured directly: retaining dirty working-tree state across retries scored **2/3 against 3/3** and cost **25.3% more input tokens per solve** (915k vs 683k). Failing turns are not hypothetical — one run recorded rollback firing on **all 30** verification failures. [FINDINGS 13.26, EXP-059, EXP-071]
3. **Context Collapse:** Production software repositories are loud, and unpredictably so. Across 64 recorded command outputs, **one** exceeded a thousand lines — and it was **25,105 lines** (`go test ./...`, exit 1): 88% of that run's entire output volume and 26× the next largest. The same command produced 961 lines in a sibling run. This is a heavy tail, not a steady flood, which is precisely why it must be handled mechanically: no rule about *which* commands are noisy would have caught it. [FINDINGS 13.26]

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

### How claims in this document are evidenced

Every empirical number here cites the `superstate-eval` entry that establishes it, so auditing
this document is a lookup rather than a reconstruction. That convention exists because it was
missing: three figures quoted as established did not survive re-measurement — a read cache's
predicted 45–68% saving, a 1,600-line read optimum fitted to a single file, and a 13,288-token
MCP overhead that re-measured at ~400 [FINDINGS 13.25, 13.20, 13.27]. All three shared one
shape: derived once, under conditions nobody recorded, then repeated.

**A number that cannot be re-derived is a claim, not a measurement.** State which it is.

Design stances — the Core Axiom, the separation of planes — are not empirical and carry no
citation. Do not confuse the two kinds of sentence.

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

**Resetting is not free, and this invariant is a default rather than a law.** One run reset a worker's *committed* feature five times, rebuilding it each time [FINDINGS 13.2]. `always` is the default because the ablation favours it [EXP-059]; `on-stall` and `on-regression` exist so the question stays measurable rather than assumed. Where the evidence is split, say so and keep the switch.

#### IV. Mechanical Perimeter Guards & AST Hygiene
You cannot regulate an agent's context through system prompt guidelines alone. Under task pressure, an agent will take the shortest path to an answer, regardless of written rules.

Context boundaries must be mechanically enforced at the execution perimeter:
- **PreToolUse Lifecycle Interception:** Runaway busy-wait polling loops must be blocked with mandatory cooldown debouncing at the system call boundary.
- **Deterministic AST Slicing:** Oversized file reads are intercepted and served as Tree-sitter symbol outlines with line numbers—zero hallucinations, zero latency. The line budget is a *measured* parameter, currently 400, not a round number [FINDINGS 13.21].
- **Tool Output Shunting:** The 25,105-line test log must be distilled to head, tail, and diagnostic signal before reaching the prompt. Note the channel this currently misses: **55–59% of a run's commands execute host-side**, outside the container bridge, and are never shunted [FINDINGS 13.21].

##### The limit of the perimeter

**A perimeter guard can shrink what a tool call returns. It cannot reduce how many calls the worker makes.** By the time a `PreToolUse` hook runs, the model has already emitted the request and the conversation has already been re-sent — the per-call cost is spent before the guard sees anything. The guard governs only the response, and therefore only what every *subsequent* turn must carry.

This matters because cost scales with call count, not response size. The consequence is not theoretical: a read cache that refused reads whose lines the worker already held was built, measured, and **increased** input tokens by 37% and 55%. 89–91% of its refusals were immediately insisted on — a worker that has decided it needs those lines needs them, and a refusal it does not accept adds a round trip instead of saving one [FINDINGS 13.25].

**So: interception is for bounding what enters context. Reducing how often the worker reaches for context is a worker-plane problem and cannot be solved here.** Any mechanism proposed under this invariant must say which of the two it is doing.

#### V. Autonomous TDD as the Problem-Solving Engine
Autonomous software engineering cannot rely on synthetic hand-authored scaffolds or answer-key leakage. Pre-seeding maintainer reproduction tests is cheating; it turns an open-ended engineering diagnosis into a narrow test-passing game.

The canonical execution engine for autonomous engineering is mechanical Test-Driven Development:
1. **Red (Milestone 1 — Reproduction):** The agent diagnoses the issue and authors an isolated reproduction test. The verification gate mechanically asserts that the test compiles and **fails** against the pristine repository. A checkpoint commit is frozen.
2. **Green (Milestone 2 — Implementation):** The agent implements the minimal fix. The gate mechanically asserts that the reproduction test **and** the full repository test suite pass (exit `0`).
3. **Commit & Handoff:** The state machine records completion, creates an atomic commit, and yields verified code.

**Known gap: this presupposes a repository exists at turn 0.** Red asserts a test fails against the pristine repository — but a task whose *instruction* is to clone or scaffold the repository has nothing to derive a runner from, and the DAG cannot be built. Failing closed is correct (Invariant II forbids a gate that cannot fail), but such a task must be recorded **ungateable for this arm**, never scored as work the agent failed to do. It was mis-scored for seven consecutive runs [FINDINGS 13.20].

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
> - **Deterministic AST slicing** over token-bloated context dumps — while remembering it
>   bounds what a call returns, never how many calls are made.
> - **Persistent environmental state** over ephemeral chat histories.
> - **Autonomous Red/Green TDD** over synthetic prompt scaffolds.

**This is Superstate.**
