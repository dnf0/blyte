# `read` Keeps Long Lines — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `read` keeps lines up to 2,000 characters whole (`READ_LINE_CHARS`) instead of clipping them at 500. Command output (`bash`, `grep`, `glob`) keeps the 500-character line cap. The 60,000-character cap per result is unchanged.

**Why (shadow run S7, 2026-09-26):** the S7 plan's two key contract lines were 640 and 578
characters long. `read` clipped both at 500, the model re-read them with `sed`, which is command
output and clipped the same way, and it spent three turns and never saw the whole spec. The
500-character cap exists for minified files and huge grep matches in command output; a file the
model explicitly asked to read is the point, not noise (the same reasoning that gave `read` its
larger line budget in #71). Markdown plans and prose routinely have long unwrapped lines.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified. `bound_output` and `bound_output_lines` keep their current behaviour (500-character lines).
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: A longer line cap for `read`

**Files:** `crates/blyte-agent/src/tools.rs`; new test `crates/blyte-agent/tests/test_read_long_lines.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_read_long_lines.rs`:

```rust
//! `read` keeps long lines whole up to READ_LINE_CHARS: a plan's 640-character contract line was
//! clipped at 500 and the model never saw it. Command output keeps MAX_LINE_CHARS.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, MAX_LINE_CHARS, READ_LINE_CHARS};
use serde_json::json;

fn write(d: &tempfile::TempDir, name: &str, body: &str) {
    std::fs::write(d.path().join(name), body).unwrap();
}

#[test]
fn read_keeps_a_long_line_whole() {
    let d = tempfile::tempdir().unwrap();
    let line = format!("start {} end", "x".repeat(1_500));
    write(&d, "plan.md", &format!("# Plan\n{line}\nlast\n"));
    let o = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({"path": "plan.md"}));
    assert!(o.content.contains(&line), "the whole 1,510-character line must come back");
    assert!(!o.content.contains("clipped by blyte"));
}

#[test]
fn read_still_clips_a_minified_line() {
    let d = tempfile::tempdir().unwrap();
    write(&d, "app.min.js", &"y".repeat(50_000));
    let o = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({"path": "app.min.js"}));
    assert!(o.content.contains("clipped by blyte"));
    assert!(o.content.chars().count() < READ_LINE_CHARS + 200, "{}", o.content.len());
}

#[test]
fn command_output_keeps_the_short_line_cap() {
    let d = tempfile::tempdir().unwrap();
    write(&d, "plan.md", &format!("{}\n", "z".repeat(1_500)));
    let o = execute(d.path(), &LocalShell, &LocalFs, "bash", &json!({"command": "cat plan.md"}));
    assert!(o.content.contains("clipped by blyte"));
    assert!(o.content.chars().count() < MAX_LINE_CHARS + 200);
    assert!(READ_LINE_CHARS > MAX_LINE_CHARS);
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_read_long_lines`. Expected: compile error (`READ_LINE_CHARS` does not exist).

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/tools.rs`:
  1. Add, next to `MAX_LINE_CHARS`:

```rust
/// Longest line `read` keeps whole. Larger than `MAX_LINE_CHARS` because a file the model asked
/// for is the point, not noise: a plan's 640-character contract line was clipped at 500 and the
/// model spent three turns failing to see it. Minified files are still clipped, here and by
/// `MAX_OUTPUT_CHARS`.
pub const READ_LINE_CHARS: usize = 2_000;
```

  2. Make the per-line cap a parameter: add `pub fn bound_output_with(text: &str, line_budget: usize, line_chars: usize) -> String` containing the current body of `bound_output_lines` with `MAX_LINE_CHARS` replaced by `line_chars`, and make `bound_output_lines(text, line_budget)` a one-line wrapper calling it with `MAX_LINE_CHARS`. Doc comment for the new function: `/// \`bound_output_lines\` with a caller-chosen line length (\`read\` uses \`READ_LINE_CHARS\`).`
  3. In `execute`, bound `read` output with `bound_output_with(&outcome.content, READ_LINE_BUDGET, READ_LINE_CHARS)` and everything else as now.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_read_long_lines --test test_bound_output --test test_read_budget --test test_read_from_top --test test_tools`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): read keeps lines up to 2,000 characters whole"
```
