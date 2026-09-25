# A Read From the Top Returns the Whole File — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** When `read` is asked for a range that starts at line 1 (`start_line` absent or 1) and the whole file fits the read budget (at most `READ_LINE_BUDGET` lines and `MAX_OUTPUT_CHARS` characters), return the whole file and ignore `end_line`. A range starting later than line 1 is honoured as before, as is any range on a file that does not fit.

**Why (shadow run S2-gate-steps, 2026-09-25):** even after `read` was allowed 2,000 lines (#71) and its description says to read whole files, the model read `run.rs` (about 700 lines) as `L1-L200`, `L201-L400`, ... six calls where one would do. Five wasted turns out of 35, each re-sending the prompt. The model pages from the top out of habit; targeted re-reads further into a file (`L470-L490` after an edit) are deliberate and stay as they are.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Whole file for a read from the top

**Files:** `crates/blyte-agent/src/tools.rs`; new test `crates/blyte-agent/tests/test_read_from_top.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_read_from_top.rs`:

```rust
//! A read from line 1 of a file that fits the read budget returns the whole file: a shadow run
//! read a 700-line file as L1-L200, L201-L400, ... six calls where one would do. Ranges that
//! start later are deliberate (a re-read after an edit) and are honoured.

use serde_json::json;
use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, READ_LINE_BUDGET};

fn file(d: &tempfile::TempDir, name: &str, lines: usize) {
    let body: String = (1..=lines).map(|i| format!("{i:08}\n")).collect();
    std::fs::write(d.path().join(name), body).unwrap();
}

fn read(d: &tempfile::TempDir, args: serde_json::Value) -> String {
    execute(d.path(), &LocalShell, &LocalFs, "read", &args).content
}

#[test]
fn a_range_from_line_one_of_a_file_that_fits_returns_the_whole_file() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let o = read(&d, json!({"path": "run.rs", "start_line": 1, "end_line": 200}));
    assert_eq!(o.lines().count(), 700);
    assert!(o.ends_with("00000700"));
}

#[test]
fn an_end_line_alone_counts_as_a_read_from_the_top() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let o = read(&d, json!({"path": "run.rs", "end_line": 50}));
    assert_eq!(o.lines().count(), 700);
}

#[test]
fn a_range_starting_later_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let o = read(&d, json!({"path": "run.rs", "start_line": 470, "end_line": 490}));
    assert_eq!(o.lines().count(), 21);
    assert!(o.contains("00000470") && o.ends_with("00000490"));
}

#[test]
fn a_range_from_the_top_of_a_file_too_big_to_fit_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "big.rs", READ_LINE_BUDGET + 500);
    let o = read(&d, json!({"path": "big.rs", "start_line": 1, "end_line": 100}));
    assert_eq!(o.lines().count(), 100);
    assert!(!o.contains("elided"));
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_read_from_top`. Expected: the first two tests fail (200 and 50 lines come back).

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/tools.rs`, in the `"read"` arm of `execute_unbounded`, replace the `let end = ...;` statement with:

```rust
            // A read from the top of a file that fits comes back whole, whatever end_line says:
            // models page from line 1 out of habit (a shadow run read 700 lines as six 200-line
            // calls). A range starting later is a deliberate re-read and is honoured.
            let fits = lines.len() <= READ_LINE_BUDGET && body.chars().count() <= MAX_OUTPUT_CHARS;
            let end = if start == 1 && fits {
                lines.len()
            } else {
                args.get("end_line")
                    .and_then(|v| v.as_u64())
                    .map(|e| e as usize)
                    .unwrap_or(lines.len())
                    .min(lines.len())
            };
```

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_read_from_top --test test_read_budget --test test_tools --test test_read_event_range`. Expected: all pass (4 new).

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): a read from line 1 of a file that fits returns the whole file"
```
