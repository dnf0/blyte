# `read` Returns Whole Files Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `read` returns up to 2,000 lines of a file at once, within the existing 60,000-character cap, instead of cutting at 400 lines. Command output (`bash`) keeps its 400-line limit.

**Why (blyte dogfood run, 2026-09-25):** 52 of 123 turns were reads, and 15 of them paged through one 650-line file (`run.rs`, 26 KB) in chunks. The 60K-character cap would have allowed one read. The 400-line limit was sized for command output (a heavy tail of build logs; superstate-eval 13.26), not for files the model asked to read. Every extra turn re-sends the whole prompt, so paging a file costs far more than reading it once.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: A larger line budget for `read`

**Files:** `crates/blyte-agent/src/tools.rs`; new test `crates/blyte-agent/tests/test_read_budget.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_read_budget.rs`:

```rust
//! `read` returns whole files up to READ_LINE_BUDGET lines (within MAX_OUTPUT_CHARS); command
//! output keeps OUTPUT_LINE_BUDGET. Paging a 650-line file cost 15 turns in a dogfood run.

use serde_json::json;
use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, MAX_OUTPUT_CHARS, OUTPUT_LINE_BUDGET, READ_LINE_BUDGET};

fn file(d: &tempfile::TempDir, name: &str, lines: usize, width: usize) {
    let body: String = (1..=lines).map(|i| format!("{i:0width$}\n")).collect();
    std::fs::write(d.path().join(name), body).unwrap();
}

#[test]
fn a_file_under_the_read_budget_comes_back_whole() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "mid.rs", 1_000, 8);
    let o = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({"path": "mid.rs"}));
    assert!(!o.content.contains("elided"), "a 1,000-line file must not be cut");
    assert_eq!(o.content.lines().count(), 1_000);
    assert!(o.content.contains("00000500") && o.content.ends_with("00001000"));
}

#[test]
fn a_file_over_the_read_budget_is_cut_with_head_and_tail() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "big.rs", READ_LINE_BUDGET + 1_000, 8);
    let o = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({"path": "big.rs"}));
    assert!(o.content.contains("elided by blyte"));
    assert!(o.content.contains("00000001"), "head kept");
    assert!(o.content.contains(&format!("{:08}", READ_LINE_BUDGET + 1_000)), "tail kept");
}

#[test]
fn the_character_cap_still_holds_for_reads() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "wide.rs", 1_500, 120);
    let o = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({"path": "wide.rs"}));
    assert!(o.content.chars().count() <= MAX_OUTPUT_CHARS + 200, "{}", o.content.len());
}

#[test]
fn command_output_keeps_the_smaller_budget() {
    let d = tempfile::tempdir().unwrap();
    let o = execute(d.path(), &LocalShell, &LocalFs, "bash", &json!({"command": "seq 1 1000"}));
    assert!(o.content.contains("elided by blyte"));
    assert!(o.content.lines().count() < OUTPUT_LINE_BUDGET + 20);
    assert!(READ_LINE_BUDGET > OUTPUT_LINE_BUDGET);
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_read_budget`. Expected: a compile error, because `READ_LINE_BUDGET` does not exist.

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/tools.rs`:
  1. After `OUTPUT_LINE_BUDGET`, add:

```rust
/// Lines a `read` returns at once, within `MAX_OUTPUT_CHARS`. Larger than `OUTPUT_LINE_BUDGET`
/// because a file the model asked for is the point, not noise: at 400 lines a dogfood run spent
/// 15 turns paging through one 650-line file, each re-sending the whole prompt.
pub const READ_LINE_BUDGET: usize = 2_000;
```

  2. Rename the body of `pub fn bound_output(text: &str) -> String` into a new `pub fn bound_output_lines(text: &str, line_budget: usize) -> String`, identical except that it calls `distil(&joined, line_budget)`. Then make `bound_output` a one-line wrapper: `bound_output_lines(text, OUTPUT_LINE_BUDGET)`. Keep `bound_output`'s doc comment, and give `bound_output_lines` the doc comment `/// \`bound_output\` with a caller-chosen line budget (\`read\` uses \`READ_LINE_BUDGET\`).`
  3. In `pub fn execute`, replace `outcome.content = bound_output(&outcome.content);` with:

```rust
    let line_budget = if name == "read" {
        READ_LINE_BUDGET
    } else {
        OUTPUT_LINE_BUDGET
    };
    outcome.content = bound_output_lines(&outcome.content, line_budget);
```

  4. In the `"read"` arm of `execute_unbounded`, change `ToolOutcome::ok(distil(&body, OUTPUT_LINE_BUDGET))` to `ToolOutcome::ok(distil(&body, READ_LINE_BUDGET))`.
  5. In `specs()`, change the `read` tool's description to: `"Read a file. Up to 2,000 lines (60,000 characters) come back in one call, so read whole files; pass start_line/end_line only for larger ones."`

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_read_budget --test test_tools --test test_bound_output`. Expected: all pass (4 new).

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): read returns whole files up to 2,000 lines, so the model stops paging"
```
