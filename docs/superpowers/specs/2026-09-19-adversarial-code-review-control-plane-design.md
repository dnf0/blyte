# Adversarial Code Review Control Plane Design

**Date:** 2026-09-19  
**Status:** Approved by User  
**Context:** `superstate` and `superstate-eval`

---

## 1. Objective

Provide a non-coercive, prompt-free verification mechanism for autonomous daemon workflows by using **adversarial git diff review** (`superstate-review`) as an autonomous quality gate.

This eliminates:
1. **Prompt Engineering on the Worker:** The coding subagent receives only the natural, raw task instruction (`task.instruction`) and environment guidance. No synthetic planning prompts, no forced formats.
2. **Fragile Hand-Authored Fixtures:** No hardcoded assertion strings (`assert b"hello"`).
3. **Vacuous Gate Exits:** When a task is ungated (verifier held out or stripped), the daemon does not prematurely commit on turn 1. Instead, the adversarial reviewer inspects the milestone's diff against the task's full objective before certifying completion.

---

## 2. Architecture & Control Flow

```
                   +---------------------------------------------+
                   |             Task Objective                  |
                   |      (Raw user instruction, unprompted)     |
                   +---------------------------------------------+
                                          |
                                          v
+---------------------------------------------------------------------------------+
| Worker Agent (Implementer)                                                      |
| - Natural coding turn with standard tool access (files, tests, shell)           |
| - Zero prompt engineering, zero few-shot coercion                              |
+---------------------------------------------------------------------------------+
                                          |
                                          | Worker exits (claims "done")
                                          v
+---------------------------------------------------------------------------------+
| Superstate Daemon Control Plane                                                 |
| 1. Evaluates Native Gate (e.g. pytest) -> if ungated, falls back to true        |
| 2. Computes Git Diff for Milestone (`get_milestone_diff`)                        |
| 3. Spawns Adversarial Reviewer:                                                 |
|    - Inputs: Full Task Goal (`state.goal`) + Milestone Git Diff                 |
|    - Schema: Structured JSON (`findings`, `severity`, `has_blocking_issues`)    |
+---------------------------------------------------------------------------------+
             |                                             |
             | [has_blocking_issues == true]               | [has_blocking_issues == false]
             v                                             v
+---------------------------------------+    +------------------------------------+
| Milestone Blocked & Iterated          |    | Milestone Certified & Advanced     |
| - Preserves working tree              |    | - Git checkpoint commit created    |
| - Review critique recorded to         |    | - Status: Completed                |
|   Epistemic Ledger (`state.notes`)    |    | - Advances to next milestone /     |
| - Worker prompted with feedback       |    |   concludes workflow               |
+---------------------------------------+    +------------------------------------+
```

---

## 3. Detailed Component Specifications

### 3.1 Superstate Core Daemon (`crates/superstate-core/src/daemon.rs`)
- **Full Goal Visibility:** When calling `superstate_review::execute_review`, pass `state.goal.as_deref().unwrap_or(&active_task.title)`. This ensures the LLM reviewer has the full task specification, rather than an 80-character truncated title.
- **Blocking on Vacuous Gate:** When the gate is vacuous (`is_vacuous_gate(verify_cmd)` is true) or `self.config.review_blocking` is true, any review report where `has_blocking_issues` is true causes `should_block = true`.
- **Epistemic Ledger Recording:** Findings are formatted into `state.notes` under `[review-advisory] Task <id>: <summary>. Issues: <list>`.
- **Working Tree Preservation for Review:** When blocking occurs due to code review, if `auto_rollback` is disabled, the working tree is kept dirty so the worker can iterate directly on its existing edits.

### 3.2 Evaluation Harness (`superstate-eval`)
- **Arm Registration:** Register/verify `superstate_daemon_review` in `lab/engine/runner.py` with:
  - `advisory_review = True`
  - `review_blocking = True`
  - `auto_rollback = False`
  - `tdd_dag = False` (pure unprompted, single-goal or natural milestone execution).
- **Parity Enforcement:** Ensure `superstate_daemon_review` satisfies all input parity checks in `tests/test_arm_input_parity.py`.
- **Zero Coercion Verification:** Confirm no hand-authored `GATE_FIXTURES` or artificial prompt templates are injected.

---

## 4. Verification Plan

1. **Unit & Integration Tests in `superstate`:**
   - Run `cargo test -p superstate-core --test test_daemon_blocking_review` confirming full goal instruction is forwarded to review.
   - Run `cargo clippy --all-targets -- -D warnings`.
2. **Integration Tests in `superstate-eval`:**
   - Run `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py tests/test_arm_input_parity.py`.
   - Run `.venv/bin/ruff check lab tests`.
3. **Empirical Canary Run:**
   - Run Canary comparison on `build-cython-ext` to verify the adversarial review catches any missing compilation or incomplete fix, guides the worker via the ledger, and passes upon clean diff.
