# Record the Turn of Each Gate Attempt — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-29):** the decider shadow trial asks how much a run spends *after* a gate failure that
was the environment's fault: that is what a "stop, it's the environment" signal would save. The run
report has each gate attempt's duration (`gate_ms`) and each turn's timing (`turn_usage`), but not
which turn each attempt happened on, so the effort after an attempt cannot be computed.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: `gate_turns`

**Files:** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/report.rs`, `docs/reports.md`,
`crates/blyte-agent/tests/test_gate_failures.rs` (a new test appended; existing tests unchanged).

- `RunReport` gains `pub gate_turns: Vec<u32>`, documented: "The turn on which each gate attempt ran,
  in order (one entry per gate attempt)." In `run_until`, push the current `turn` where
  `report.gate_attempts += 1` is done.
- The report JSON (`crates/blyte-cli/src/report.rs`) gains `gate_turns` next to `gate_ms`, copied from
  the run report, with `#[serde(default)]` so older reports still load. Every field is always written,
  as for the others. `SCHEMA_VERSION` does not change (the field is additive).
- `docs/reports.md`: add `gate_turns` to the field list next to `gate_ms`, with the sentence above.

- [ ] **Step 1: test** (append to `test_gate_failures.rs`, using its `run` helper):

```rust
#[test]
fn each_gate_attempt_records_its_turn() {
    let rep = run("echo boom-marker && exit 1", 2);
    assert_eq!(rep.gate_turns.len(), rep.gate_attempts as usize);
    assert!(rep.gate_turns.windows(2).all(|w| w[0] < w[1]), "{:?}", rep.gate_turns);
    assert!(*rep.gate_turns.last().unwrap() <= rep.outcome_turns);
}
```

- [ ] Steps 2–4: fail, implement, full checks. **Step 5: Commit.**

```bash
git add -A crates docs/reports.md
git commit --no-gpg-sign -m "feat(agent): the run report records the turn of each gate attempt"
```
