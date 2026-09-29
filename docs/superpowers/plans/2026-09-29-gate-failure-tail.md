# Keep the End of Gate Failures in the Run Report — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Why (2026-09-29):** `RunReport::gate_failures` keeps only the first 4,000 characters of each failed
gate's output. On a large Rust workspace those are all passing-test lines, so the failure itself is
lost from the report: all 8 real gate failures recorded so far were unreadable for this reason
(decider trial, blyte-eval `studies/decider-2026-09-28/`). The model is unaffected (it sees the
bounded head and tail); the report, which orchestrators and studies read, is what loses it.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified, **except**
  `long_output_is_cut` in `crates/blyte-agent/tests/test_gate_failures.rs`, which encodes the old
  head-only rule and is replaced as below.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: head and tail

**File:** `crates/blyte-agent/src/run.rs` (`cut_gate_output`), `crates/blyte-agent/tests/test_gate_failures.rs`.

`cut_gate_output(output)`:
- at most 4,000 characters: returned unchanged;
- longer: the head (the first 1,000 characters, shortened to end at the last newline inside them if
  there is one), then the line `… [N characters cut] …` (N = characters dropped), then the tail (the
  last 3,000 characters, shortened to start after the first newline inside them if there is one),
  joined with newlines. Count characters, not bytes (the output may hold multi-byte text), and
  never split a character.

Update the doc comment on `gate_failures` in `RunReport` ("bounded to 4,000 characters") to say
it keeps the head and the tail.

- [ ] **Step 1: tests.** In `test_gate_failures.rs`, replace `long_output_is_cut` with:

```rust
#[test]
fn long_output_keeps_the_head_and_the_tail() {
    // The failure is usually reported last; the first line names the failing steps.
    let rep = run(
        "for i in $(seq 1 300); do echo \"line $i $(printf 'x%.0s' $(seq 1 60))\"; done; echo final-failure-marker; exit 1",
        1,
    );
    let g = &rep.gate_failures[0];
    assert!(g.chars().count() <= 4_100, "{}", g.chars().count());
    assert!(g.contains("line 1 "), "head kept");
    assert!(g.contains("final-failure-marker"), "tail kept");
    assert!(g.contains("characters cut] …"), "the cut is marked");
    assert!(!g.contains("line 150 "), "the middle is dropped");
}

#[test]
fn multi_byte_output_is_cut_on_character_boundaries() {
    let rep = run("for i in $(seq 1 2000); do echo 'é€'; done; exit 1", 1);
    let g = &rep.gate_failures[0];
    assert!(g.chars().count() <= 4_100);
    assert!(g.contains("characters cut] …"));
}
```

- [ ] Steps 2–4: fail, implement, full checks. **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "fix(agent): the run report keeps the head and the tail of a failed gate's output"
```
