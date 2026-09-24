# Blyte Daemon Capabilities Roadmap

**Date:** 2026-09-19  
**Status:** Approved & Prioritized  
**Context:** Following the validation of the single-milestone autonomous control plane in EXP-062 (blocking review, deterministic git rollback, and native tool output shunting), this roadmap charts the sequence for unlocking the full multi-milestone control plane in `blyte`.

---

## 1. Architectural Strategy & Grounding Principles

### What the Evaluation Proved (300+ Runs)
1. **Markdown Plans Were Prompt Engineering:** Upfront LLM planning turns (`### Task 1: Title`, `- Depends on:`, regex scraping) added latency, token bloat, and hallucinated decompositions before inspecting code.
2. **What Actually Delivered Lift:** Deterministic runtime primitives:
   - External verification gates (exit codes decide truth, not model self-assertion).
   - Deterministic Git rollback on gate failure (clean baselines on retry).
   - Tool output shunting (preventing context window saturation).
   - Detached OS daemon runner (driving iteration outside LLM context loops).
3. **Core Ethos:** Blyte is an external, deterministic verification runtime. It executes against verifiable gates provided by the human or orchestrator; it must never synthesize its own answer keys or grade its own homework.

---

## 2. Prioritized Initiatives & Time Breakdown

```
┌──────────────────────────────────────────────┐
│  Initiative 1: Real Dependency DAG (~3h)     │ ◄── [ACTIVE]
│  Topological task selection, milestone commit│
│  baselines, multi-step gate progression      │
└──────────────────────┬───────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────┐
│  Initiative 2: Test Impact Slicing (~5h)     │ ◄── [QUEUED]
│  AST Tree-sitter diff extraction, fast slice │
│  intermediate gates, full suite on advance   │
└──────────────────────┬───────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────┐
│  Initiative 3: Native Git Worktrees (~7h)    │ ◄── [QUEUED]
│  Sandboxed .wt/<task-id>, cache reflink/link,│
│  zero negative work, atomic branch merging   │
└──────────────────────────────────────────────┘
```

---

### Initiative 1: Real Dependency DAG Execution in the Daemon
* **Status:** **ACTIVE**
* **Estimated Time:** **~3 hours**
  - Design & Spec: 30m
  - Implementation in `daemon.rs`: 1h 15m
  - TDD Integration Test Suite: 45m
  - Review & PR Integration: 30m
* **Problem:** In `daemon.rs`, multi-milestone advancement is currently hardcoded to `pending_tasks.remove(0)` (blind file order), ignoring `TaskSpec.depends_on`. While `advance_step()` in `state_machine.rs` models DAG resolution, `DaemonRunner` never wired it in.
* **Scope:**
  - Replace `remove(0)` with unblocked DAG task resolution (`ready_tasks`).
  - Milestone baseline tracking: each milestone tracks its own base commit, retry count, and failure diagnostics.
  - Multi-milestone rollback: rolling back a failed attempt on Milestone 2 reverts to Milestone 1's completion commit, never wiping Milestone 1's verified work.
  - Integration tests for sequential, parallel/independent, and diamond DAG workflows.

---

### Initiative 2: Milestone-Scoped Test Impact Slicing (`impact.rs`)
* **Status:** **QUEUED (Follows Initiative 1)**
* **Estimated Time:** **~5 hours**
  - Design & Spec: 45m
  - Implementation (test filter adapters & staged gating): 2h 30m
  - TDD Multi-language Test Suite: 1h 15m
  - Review & PR: 30m
* **Problem:** ADR-0001 specified progressive verification, and `crates/blyte-core/src/impact.rs` implements 346 lines of Tree-sitter AST symbol extraction for Rust, Python, Go, and TypeScript. However, nothing in the execution loop invokes it.
* **Scope:**
  - Language adapters translating AST diffs into test-runner filters (`pytest -k`, `cargo test <symbol>`, `go test -run`).
  - Staged gating in `daemon.rs`: run fast impacted slice on intermediate turns; run full milestone verification gate before advancement.
  - Robust fail-closed fallback to full test command if AST parsing encounters syntax errors.

---

### Initiative 3: Native Git Worktree Isolation & Pareto Monotonicity
* **Status:** **QUEUED (Follows Initiative 2)**
* **Estimated Time:** **~7 hours**
  - Design & Spec (cache sharing & merge/prune contracts): 1h
  - Implementation (`WorktreeManager` + cache symlinks + daemon lifecycle): 3h 30m
  - Integration Testing (concurrency, crash recovery, dirty base branches): 1h 30m
  - Review & PR: 45m
* **Problem:** The product promise of "Zero Negative Work" requires that an agent never leaves `main` or the host checkout in a broken state. In early versions, worktrees were provisioned externally or disabled (`no_worktree: true`).
* **Scope:**
  - Native `WorktreeManager` in `blyte-core` managing copy-on-write worktrees (`.wt/<task-id>`).
  - Dependency caching: symlink or reflink heavy directories (`node_modules/`, `target/`, `.venv/`) to avoid install latency.
  - Autonomous merge/prune lifecycle: fast-forward or squash merge into target branch on milestone completion; instantly prune worktree on failure or abort.
