# Superstate Micro-Turn Optimization Implementation Plan

Decomposed state-machine turns should be small, laser-focused, and fast (30–45s). Currently, turns take 180s–280s because of over-decomposition of atomic tasks, synthetic test-suite generation tax, and epistemic ledger prompt accumulation. This plan optimizes planning granularity, contracts, and ledger compaction to achieve fast micro-turns and beat vanilla on simple tasks.

## Proposed Changes

### Task 1: Compact Epistemic Ledger Formatting
- In `crates/superstate-cli/src/commands/state.rs`: format ledger entries compactly in `cmd_prompt`: truncate multiline prose notes to clean single-paragraph summaries (max 250 chars) and show only the most recent 3–4 entries if the ledger grows.
- In `lab/engine/adapters/superstate.py`: distill `last_response` before recording to ledger: extract the `### Summary` section or first 250 characters rather than dumping 1,500 characters of raw markdown.

### Task 2: Minimal Direct Execution Contract
- In `lab/engine/adapters/superstate.py`:
  - Add Rule 1: **MINIMAL DIRECT FIX**: Write ONLY the minimal code required to pass the verification gate. Do NOT build speculative CLI flags, general-purpose frameworks, or unnecessary helper utilities.
  - Clarify: Do not author synthetic test suites from scratch if the verification command can verify the deliverable directly.

### Task 3: Adaptive Plan Granularity & Direct Invariant Verification
- In `lab/engine/adapters/superstate.py`:
  - **Adaptive Granularity:** Instruct the planner that atomic, script-based, or localized tasks should have at most 1–2 milestones. Do NOT over-decompose into 4+ milestones unless the problem spans distinct subsystem boundaries.
  - **Direct Invariant Verification:** Allow `- Test: <none or existing>` and encourage direct verification commands (e.g. `python3 -c "..."` or checking target file output) rather than requiring the planner to invent non-existent test files.

### Task 4: Empirical Benchmark Validation
- Run paired benchmark on `terminal-bench/db-wal-recovery` with `superstate_rust_inplace`.
- Verify turn duration drops significantly, token count drops, and tests pass 7/7.
