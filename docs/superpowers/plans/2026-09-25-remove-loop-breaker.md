# Remove the Loop-Breaker Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Delete the loop-breaker and everything that exists only to feed it:
- the redundant-read and redundant-command guards;
- the insist escape;
- the working set;
- the action ledger;
- phase markers in the ledger;
- the run report's interception and insist counts;
- `--no-loop-breaker`.

**Why (user decision, 2026-09-25):** It was built for `StateOnly`, where the model could not see earlier results. Under `folded` it fired 24 times in 8,463 turns (0.3%), with 0 insists, across 42 runs. Its messages point at an "Active Working Set" section of the prompt that only the deleted `StateOnly` rendered.

**Stays:** `extract_action_target` and `extract_action_summary` (the fold digest uses them), everything in the agent loop that is not a guard, `run_phases` and its gates, the event log, per-turn usage, and the run report's other fields.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/remove-loop-breaker` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- Do not run Docker. `Cargo.lock` is gitignored.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass.
- This is a deletion. Tests whose subject is deleted are deleted with it. Tests that only mention a deleted field incidentally (a `Runner { loop_breaker: ... }` literal, an `ActionEntry` in a fixture of an unrelated test) are edited minimally. **List every test file deleted and every test edited in your report.** Do not weaken assertions about features that stay.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Remove the loop-breaker, working set and action ledger

**Files:** `crates/blyte-agent/src/{run.rs,state.rs,lib.rs,working_set.rs}`, `crates/blyte-agent/examples/{harness_bench.rs,solve.rs}`, `crates/blyte-cli/src/{cli.rs,task_runner.rs,report.rs,repl.rs}`, and tests.

- [ ] **Step 1: Agent.**
  - Delete `crates/blyte-agent/src/working_set.rs` and `pub mod working_set;`.
  - In `state.rs`, delete `ActionEntry`, `Runtime.actions` and `Runtime.working_set`, and any docs that describe them.
  - In `run.rs`:
    - delete the `loop_breaker` field of `Runner` (with its doc comment);
    - delete `PHASE_MARKER_TOOL`, `current_phase_actions`, `verification_passed_since_edit`, `is_insist`, and every `should_intercept_*` function, along with any other function used only by them;
    - delete the phase-marker push in `run_phases`;
    - delete, in `run_until`, the guard evaluation (`guards_on` and the interception branches), both `state.runtime.actions.push(...)` calls, and every `state.runtime.working_set.*` update.
    
    A tool call is now always executed: `note` is handled as before, and everything else goes to `tools::execute`. Keep emitting `Event::Tool` exactly as now.
  - Remove `loop_breaker` from the `Runner` literals in `examples/harness_bench.rs` and `examples/solve.rs`.
- [ ] **Step 2: CLI.**
  - Delete the `--no-loop-breaker` flag (`no_loop_breaker`), the `TaskConfig.loop_breaker` field and its `From<&Cli>` line, and the `loop_breaker:` line of the `Runner` literal in `task_runner.rs`.
  - In `report.rs`, delete `RunRecord.loop_breaker`, `interceptions` and `insists`, the `Interceptions` type, `count_interceptions`, `count_insists`, and the now-unused `ActionEntry` import.
  - In `repl.rs`, remove the `/state` output's working-set and last-failure sections (the rest of `/state` stays), and remove `loop_breaker` from its `TaskConfig` literal.
- [ ] **Step 3: Tests.** Remove every `loop_breaker: <expr>,` line from `Runner` literals, and every `loop_breaker: <expr>,` field from `TaskConfig` literals, across `crates/*/tests/` and `crates/*/examples/`. For example:

```bash
git grep -l "loop_breaker:" -- crates | xargs perl -0pi -e 's/\n[ \t]*loop_breaker: [^\n]*,\n/\n/g'
```

  Then:
  - **Delete** test files whose subject is deleted: `crates/blyte-agent/tests/test_loop_breaker_honesty.rs`, `crates/blyte-agent/tests/test_working_set.rs`, `crates/blyte-cli/tests/test_working_set_e2e.rs`, `crates/blyte-cli/tests/test_action_ledger_e2e.rs`.
  - **In `test_phases.rs`**, delete the test that asserts phase markers in the ledger.
  - **In `test_run.rs`, `test_state.rs`, `test_run_report.rs` and any other file** that still fails to compile: delete tests of guards, the ledger, the working set, interceptions or insists, and edit incidental references minimally.
- [ ] **Step 4: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Then:

```bash
git grep -n -E "loop_breaker|working_set|WorkingSet|ActionEntry|runtime\.actions|is_insist|should_intercept|verification_passed_since_edit|current_phase_actions|PHASE_MARKER_TOOL|count_interceptions|count_insists|no-loop-breaker|Active Working Set|Working Memory Guard" -- crates/
```

  Expected: no output. Report the deleted test files, the edited tests, and `find crates -name '*.rs' | xargs wc -l | tail -1`.
- [ ] **Step 5: Commit.**

```bash
git add -A crates/
git commit --no-gpg-sign -m "refactor!: remove the loop-breaker, working set and action ledger"
```
