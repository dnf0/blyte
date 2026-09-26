# Stale Descriptions — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** `blyte --help` and the CLI crate's description say what blyte is now. Both still describe the removed bolt-on era ("deterministic state", "state engine runner").

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Change only the two strings below and add the test. Do not change the crate version or anything else in `Cargo.toml`.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Update the two descriptions

**Files:** `crates/blyte-cli/src/cli.rs`; `crates/blyte-cli/Cargo.toml`; new test `crates/blyte-cli/tests/test_descriptions.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-cli/tests/test_descriptions.rs`:

```rust
//! The help text and crate description describe blyte as it is, not the removed state engine.

use std::process::Command;

#[test]
fn help_describes_the_harness() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("A lean coding-agent harness with a sealed verification gate"), "{help}");
    assert!(!help.contains("deterministic state"), "{help}");
}

#[test]
fn the_crate_description_no_longer_mentions_a_state_engine() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains(
        "description = \"The blyte coding-agent harness: headless runs with a sealed verification gate, and a REPL.\""
    ));
    assert!(!manifest.contains("state engine"));
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte --test test_descriptions`. Expected: both fail.

- [ ] **Step 3: Implement.**
  1. In `crates/blyte-cli/src/cli.rs`, change `about = "Blyte: Autonomous agent harness with deterministic state and verification gates"` to `about = "A lean coding-agent harness with a sealed verification gate"`.
  2. In `crates/blyte-cli/Cargo.toml`, change `description = "Blyte unified interactive agent harness and state engine runner."` to `description = "The blyte coding-agent harness: headless runs with a sealed verification gate, and a REPL."`.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte --test test_descriptions --test test_cli_smoke`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "docs(cli): help text and crate description describe blyte as it is"
```
