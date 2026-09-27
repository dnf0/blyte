# The Shell Session Heals Its Temp Directory — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** blyte's persistent shell session keeps working after a command deletes the session's own
temporary directory: the directory is recreated before every command.

**Why (shadow run S15, 2026-09-27):** blyte's persistent session writes each command's stdout and
stderr to files in a temporary directory it creates once at start-up (`__blyte_t`). If a command
deletes that directory, every later command's output redirect fails and reports exit 1, even `pwd`
or `echo`, and the session never recovers. In S15 blyte's model, while experimenting with temp
directories and `sandbox-exec`, broke its session this way and spent its remaining ~250 turns
unable to run anything, exhausting the 300-turn budget with no commit. Reproduced directly: after
`rm -rf "$__blyte_t"`, the next `echo after` exited 1.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Use the directory path the Rust side recorded at start-up (the session's `tmp` field), not the
  shell variable `$__blyte_t`, which a command could change or unset.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Recreate the temp directory before each command

**Files:** `crates/blyte-agent/src/shell.rs`; new test `crates/blyte-agent/tests/test_session_self_heal.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_session_self_heal.rs`:

```rust
//! A command that deletes the session's temp directory must not break every later command: in a
//! shadow run the model did exactly that and spent 250 turns unable to run even `pwd`.

use std::time::Duration;

use blyte_agent::shell::{PersistentShell, Shell};

#[test]
fn the_session_survives_its_temp_directory_being_deleted() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = PersistentShell::local(root.clone());
    let t = Duration::from_secs(30);

    let before = s.exec_user("echo before", &root, t).unwrap();
    assert_eq!(before.exit_code, 0);

    let rm = s.exec_user(r#"rm -rf "$__blyte_t""#, &root, t).unwrap();
    assert_eq!(rm.exit_code, 0);

    let after = s.exec_user("echo after", &root, t).unwrap();
    assert_eq!(after.exit_code, 0, "{}", after.merged());
    assert!(after.merged().contains("after"));
}

#[test]
fn the_session_survives_the_variable_being_unset_too() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = PersistentShell::local(root.clone());
    let t = Duration::from_secs(30);
    s.exec_user(r#"rm -rf "$__blyte_t"; unset __blyte_t"#, &root, t).unwrap();
    let after = s.exec_user("echo still-here", &root, t).unwrap();
    assert_eq!(after.exit_code, 0, "{}", after.merged());
    assert!(after.merged().contains("still-here"));
}
```

  If `PersistentShell::local` or `exec_user` have slightly different signatures, adapt only those
  calls (see `crates/blyte-agent/tests/test_persistent_shell.rs` for how sessions are made and used);
  keep every assertion.

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_session_self_heal`. Expected: both tests fail (exit 1 after the deletion).

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/shell.rs`, in the persistent session's
  per-command script (the `run` method that builds the heredoc wrapper), begin the command's line
  with `mkdir -p <quoted tmp> 2>/dev/null;` using the session's recorded `tmp` path, so the directory
  exists before the output redirects. Add a one-line comment saying why (the Why above, briefly).

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_session_self_heal --test test_persistent_shell --test test_session_protocol --test test_tools`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "fix(agent): the shell session recreates its temp directory before each command"
```
