# Design Specification: The Superstate Manifesto

**Date:** 2026-09-20  
**Status:** Approved  
**Target File:** `MANIFESTO.md` in repository root  
**Secondary Targets:** Cross-link in `README.md` and `docs/index.md`

---

## 1. Executive Summary & Purpose

The **Superstate Manifesto** establishes the foundational doctrine of autonomous software engineering. 

Current AI coding tools (Claude Code, Cursor, Copilot, raw Agy) treat software engineering as an extended conversational chat session. In empirical software engineering benchmarks (SWE-bench, DataCurve, terminal-bench), conversational execution fails on production repositories: workers declare victory while wrong in 83% of losses, poison git trees with cascading unverified edits, and blow out context windows with noisy compiler and test dumps.

The Manifesto codifies why conversational development fails, establishes the separation between the untrusted **Worker Plane** and the deterministic **Control Plane**, and details the **Five Invariants of Autonomous Execution** that define Superstate.

---

## 2. Document Architecture & Content Requirements

The manifesto will be authored in `/Users/danielfisher/repos/superstate/MANIFESTO.md` with the following canonical structure:

### 2.1 The Fallacy of Chat-Driven Development
- **Premise:** Software engineering is a stateful systems discipline, not next-token prediction.
- **The Three Structural Failures of Conversational Agents:**
  1. *The Confidence Trap (Done While Wrong):* LLMs have no mechanical friction; subjective confidence does not correlate with working code.
  2. *Cascading Edit Pollution:* Failed attempts accumulate in the working tree without rollback, confusing the agent on subsequent turns.
  3. *Context Collapse:* Unfiltered compiler dumps, test logs, and runaway polling saturate token buffers and trigger prompt amnesia.

### 2.2 The Core Axiom
> **Autonomous software engineering requires a strict decoupling between the Worker Plane and the Control Plane.**
> The model is an untrusted, interchangeable reasoning worker. The control plane is a deterministic, unyielding supervisor.
> The worker proposes changes. The environment decides truth.

### 2.3 The Five Invariants of Autonomous Execution
1. **Invariant I: State Belongs to the Environment, Not the Token Buffer**
   - Context is lossy and ephemeral.
   - Ground truth resides strictly in the filesystem, git commit DAG, dependency graph, and external state machine (`.agent-state/state.json`).
   - The model remains stateless; the supervisor holds state.
2. **Invariant II: Mechanical Verification Over Self-Assessment**
   - "Done" is not a qualitative feeling or LLM reflection; it is a binary shell exit code certified by a verifier.
   - Vacuous gates (`exit 0`, `true`) certify nothing. If the gate fails, the worker cannot proceed or merge.
3. **Invariant III: Scoped Git Rollbacks Over Cascading Edit Pollution**
   - Unverified edits must never accumulate across failed turns.
   - Milestone boundaries create immutable git checkpoints.
   - On gate failure, the control plane executes `git reset --hard <checkpoint>`, ensuring fresh turns begin from a pristine baseline with precise failure telemetry.
4. **Invariant IV: Mechanical Perimeter Guards & AST Hygiene**
   - Instructions alone fail under pressure; enforcement must occur at the execution boundary.
   - PreToolUse hooks debounce runaway polling loops (30s cooldown).
   - Compiled Tree-sitter AST parsing slices oversized files into exact symbol outlines and line numbers without LLM hallucination.
   - Tool output shunting distills noisy build and test outputs into diagnostic head/tail summaries.
5. **Invariant V: Autonomous TDD as the Problem-Solving Engine**
   - Answer-key leakage and synthetic prompt scaffolds are rejected.
   - The canonical loop is Red (author reproduction test, verify failure against pristine repo, checkpoint) $\rightarrow$ Green (implement fix, verify reproduction test and full suite pass) $\rightarrow$ Commit.

### 2.4 The Architecture of Separation
- Diagram contrasting the **Control Plane** (Superstate: Rust daemon, milestone DAG, exit code evaluator, rollback engine, AST slicer, lifecycle guards) and the **Worker Plane** (Interchangeable LLMs/drivers: Antigravity, Claude Code, OpenAI).

### 2.5 The Value Trade-offs & Standard
- Summary value statements:
  - *Mechanical verification* over *agent self-confidence*.
  - *Scoped git rollbacks* over *conversational apologies*.
  - *Deterministic AST slicing* over *token-bloated context dumps*.
  - *Persistent environmental state* over *ephemeral chat histories*.
  - *Autonomous Red/Green TDD* over *synthetic prompt scaffolds*.

---

## 3. Integration & Cross-References

- In `README.md`: Add a prominent callout block linking to `[MANIFESTO.md](MANIFESTO.md)` in the introduction.
- In `docs/index.md`: Link to the Manifesto as the core philosophical statement of the project.

---

## 4. Verification & Criteria of Success

1. `MANIFESTO.md` exists in repository root, fully written with zero placeholders or TODOs.
2. All markdown formatting, code blocks, and diagrams render cleanly.
3. `README.md` and `docs/index.md` reference `MANIFESTO.md` with valid relative links.
4. Clean git commit on `docs/superstate-manifesto` following conventional commits (`--no-gpg-sign`).
