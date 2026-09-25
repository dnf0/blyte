# Remove `--review` and `--tdd` Implementation Plan

> **For agentic workers:** This plan is the first dispatched to **blyte itself** (dogfooding) instead of agy. Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Delete the completeness review (`--review`) and autonomous TDD (`--tdd`), with everything that exists only to serve them:
- the review call and its prompt;
- multi-phase runs (`run_phases`, `Phase`, checkpoint commits, and the scope and protect gates);
- the CLI's TDD wording;
- the run report's review and phase fields.

**Why (user decision, 2026-09-25):** blyte's job is to implement plans for a Claude Code orchestrator. The orchestrator writes the plan (tests included) and reviews each result independently, so completeness is its job. Neither feature showed a measurable effect (superstate-eval 13.40, n=1). The objective is speed without bloat.

**Stays:** the single verification gate (`--gate`), rollback to the baseline commit on gate failure (`checkpoint` / `reset_to`), the loop, folded and conversational context, tools, sessions, native search, pricing, per-turn usage, and the rest of the run report.

**Tech Stack:** Rust 2021.

## Global Constraints

- The task runs in its own git worktree, already on its own branch. Work only there.
- This is a deletion. Tests whose subject is deleted are deleted with it; incidental references are edited minimally. Do not weaken assertions about anything that stays.
- The harness runs `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` as the gate when you stop. You may run it or targeted tests yourself before that.
- Commit with `--no-gpg-sign` and the exact message below, before you stop.

---

### Task 1: Remove the review gate and TDD phases

**Files:** `crates/blyte-agent/src/{run.rs,state.rs}`, `crates/blyte-cli/src/{cli.rs,task_runner.rs,report.rs,lib.rs,tdd.rs,repl.rs}`, and tests.

- [ ] **Step 1: Agent (`run.rs`, `state.rs`).**
  - Delete the review: `fn review`, `REVIEW_SYSTEM`, `MAX_REVIEW_ROUNDS`, `ReviewVerdict`, `ReviewFinding`, the review block in `run_until` (the gate-pass path then simply returns `Outcome::Verified`), and `RunReport.reviews` / `review_rejections`.
  - Delete `Orchestrator.review` (with its doc) from `state.rs`, and its initialisation in `State::new`.
  - Delete multi-phase runs: `pub struct Phase`, `run_phases`, `PhaseResult` (or whatever `run_phases` returns per phase), `commit_checkpoint`, and the scope-only and protect gate logic, with any helpers used only by them.
  - Keep `checkpoint` and `reset_to`: `run_until` uses them for rollback.
- [ ] **Step 2: CLI.**
  - Delete the `--tdd` and `--review` flags, and the `TaskConfig.tdd` / `review` fields and their `From<&Cli>` lines.
  - In `task_runner.rs`, delete `tdd_phases`, the `phases` computation, `state.orchestrator.review = ...`, and the `run_phases` branch. The run is always `runner.run(state)`.
  - Delete `crates/blyte-cli/src/tdd.rs` and `pub mod tdd;`.
  - In `report.rs`, delete `reviews`, `review_rejections`, `phases`, `PhaseRecord`, `with_phases` and anything only they use.
  - `gate_source` becomes `"user"` when `--gate` is set and `"none"` otherwise, and `gated` is `gate.is_some_and(non-empty)`.
  - Remove the removed fields from the REPL's `TaskConfig` literal.
- [ ] **Step 3: Tests.**
  - Delete `crates/blyte-agent/tests/test_review_gate.rs`, `crates/blyte-agent/tests/test_phases.rs` and `crates/blyte-cli/tests/test_tdd_phases.rs`.
  - In `test_run_report.rs`, `test_task_runner.rs`, `test_cli_dispatch.rs` and any other file that no longer compiles, remove the deleted fields and flags from literals and assertions, and delete tests whose only subject was review or TDD.
- [ ] **Step 4: Check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Then:

```bash
git grep -n -E "review_rejections|REVIEW_SYSTEM|MAX_REVIEW_ROUNDS|fn review\b|orchestrator\.review|run_phases|struct Phase\b|PhaseRecord|tdd_phases|--tdd|--review|commit_checkpoint|scope_only" -- crates/
```

  Expected: no output.
- [ ] **Step 5: Commit.**

```bash
git add -A crates/
git commit --no-gpg-sign -m "refactor!: remove --review and --tdd (the orchestrator owns completeness)"
```
