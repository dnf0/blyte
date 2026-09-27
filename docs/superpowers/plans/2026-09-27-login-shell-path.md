# Keep the Inherited PATH in Front — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** commands blyte runs on the host find the same programs first as the shell blyte was
started from. The login shell's own PATH additions are kept, after the inherited ones.

**Why (2026-09-27):** blyte runs host commands in `bash -l`. On macOS the login profile
(`/etc/profile`) runs `path_helper`, which moves system directories to the front of PATH. A user
with pyenv (initialised in `~/.zshrc`) started blyte with `~/.pyenv/shims` first, but inside blyte
`python3` was `/usr/local/bin/python3`, without pytest: a real dispatch spent all 20 turns
investigating why `python3 -m pytest` failed. Rust tooling was unaffected (`~/.cargo/bin` has no
system twin), which is why this went unnoticed.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**.
- Container commands are unchanged (they never see the host PATH).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Restore the inherited PATH after the login profile

**Files:** `crates/blyte-agent/src/shell.rs`, `crates/blyte-agent/tests/test_inherited_path.rs` (new).

**What to build:**

- `pub fn inherited_path_prefix() -> String` in `shell.rs`: when this process's `PATH` is set and
  non-empty, `export PATH=<PATH, single-quoted with the module's existing quoting helper>:"$PATH"; `
  (note the trailing space); otherwise the empty string. Reading `PATH` happens at call time.
- `LocalShell::exec`: every host variant (unsandboxed, Seatbelt, Landlock, LandlockOffline) runs
  `inherited_path_prefix() + command` instead of `command`.
- The persistent session (`Launcher::Local` only): the initialisation script sent in
  `Session::start` begins with `inherited_path_prefix()`, so every later command in the session
  sees the restored PATH. `Launcher::Container` is unchanged.
- A one-line doc comment on `inherited_path_prefix` saying why (macOS `path_helper` in the login
  profile reorders PATH).

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_inherited_path.rs`:

```rust
//! Commands blyte runs find the programs the user's own shell finds first: on macOS the login
//! profile's path_helper moved /usr/local/bin ahead of ~/.pyenv/shims, so `python3` inside blyte
//! lacked pytest and a dispatch spent 20 turns on it (2026-09-27).

use std::time::Duration;

use blyte_agent::shell::{inherited_path_prefix, LocalShell, PersistentShell, Shell};

// One test, because it changes this process's PATH.
#[test]
fn a_program_first_on_the_inherited_path_wins_in_commands_and_sessions() {
    let bin = tempfile::tempdir().unwrap();
    // A fake `ls`: the system one lives in /bin, which a login profile may move to the front.
    let fake = bin.path().join("ls");
    std::fs::write(&fake, "#!/bin/sh\necho fake-ls\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let old = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{old}", bin.path().display()));

    let prefix = inherited_path_prefix();
    assert!(prefix.starts_with("export PATH="), "{prefix}");
    assert!(prefix.ends_with(":\"$PATH\"; "), "{prefix}");

    let ws = tempfile::tempdir().unwrap();
    let out = LocalShell.exec("ls", ws.path(), Duration::from_secs(30)).unwrap();
    assert_eq!(out.stdout.trim(), "fake-ls", "{}", out.merged());

    let session = PersistentShell::local(ws.path().to_path_buf());
    let out = session.exec("ls", ws.path(), Duration::from_secs(30)).unwrap();
    assert_eq!(out.stdout.trim(), "fake-ls", "{}", out.merged());

    std::env::set_var("PATH", old);
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_inherited_path`: compile error (`inherited_path_prefix` missing). On macOS, once it compiles and before the change, the `ls` assertions fail with the real `ls` output.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "fix(agent): commands keep the inherited PATH in front of the login profile's"
```
