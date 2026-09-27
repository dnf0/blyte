# Linux: Commands Read Only What They Need — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not run Docker. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** on Linux, blyte's sandboxed commands can read only system directories, the workspace
(and the other write-allowed paths), named tool directories under `$HOME`, and `--allow-read`
grants. Everything else under `$HOME` (`~/.ssh`, `~/.aws`, `~/.config/gh`, `~/.codex`, keyrings,
browser profiles, other checkouts, …) is unreadable. This closes the biggest gap with macOS,
where credential reads are already refused.

**Why (2026-09-27):** Landlock cannot deny a path inside an allowed tree, which is why the macOS
read-deny was never ported; bubblewrap needs unprivileged user namespaces, which Ubuntu 24.04
(including GitHub's runners) restricts. But Landlock can **allow-list** reads: handle the read
rights and allow them only beneath the listed paths. Omission does the denying.

**Tech Stack:** Rust, `landlock` 0.4.7 (already a Linux dependency).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**, with two named exceptions, each only in the Linux
  (non-macOS) branch of the test:
  1. `crates/blyte-agent/tests/test_sandbox_linux.rs`, `the_default_profile_matches_the_platform`:
     the non-macOS arm matches `Profile::LandlockConfined { write, read, block_tcp: false }` and
     asserts `write` contains the temp dir and `/dev`, and `read` contains `/usr`.
  2. `crates/blyte-agent/tests/test_network_off.rs`, `the_offline_default_profile_matches_the_platform`:
     the non-macOS arm matches `Profile::LandlockConfined { write, block_tcp: true, .. }` and asserts
     `write` contains the temp dir.
- The `Profile::Landlock` and `Profile::LandlockOffline` variants stay, with their current
  behaviour (write-only), for anyone building them directly.
- macOS behaviour is unchanged.
- Checks, all must pass:
  - `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
  - `CC_aarch64_unknown_linux_gnu=zigcc-aarch64-linux cargo clippy --target aarch64-unknown-linux-gnu --workspace --all-targets -- -D warnings`
  (the Linux tests run in CI on Ubuntu, where Landlock is required to be present).
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `Profile::LandlockConfined` and the read allowlist

**Files:** `crates/blyte-agent/src/sandbox.rs`, `crates/blyte-agent/src/shell.rs`,
`crates/blyte-cli/src/task_runner.rs`, `crates/blyte-cli/src/report.rs`,
`crates/blyte-agent/tests/test_linux_read_allowlist.rs` (new), and the two named test changes.

**What to build:**

- `sandbox.rs`:
  - New variant `Profile::LandlockConfined { write: Vec<PathBuf>, read: Vec<PathBuf>, block_tcp: bool }`.
  - `pub fn read_allowed(home: Option<&Path>, allow_read: &[PathBuf]) -> Vec<PathBuf>`, in this order:
    the system roots `/usr`, `/lib`, `/lib32`, `/lib64`, `/libx32`, `/bin`, `/sbin`, `/etc`, `/opt`,
    `/proc`, `/sys`, `/dev`, `/run`, `/var`, `/tmp`, `/snap`, `/nix`; then, when `home` is given,
    `home` joined with each of `.cargo`, `.rustup`, `.cache`, `.local/bin`, `.local/lib`,
    `.local/share/uv`, `.local/share/pnpm`, `.local/share/mise`, `.npm`, `.nvm`, `.pyenv`, `.rbenv`,
    `.asdf`, `.bun`, `.deno`, `.gradle`, `.m2`, `go`, `.config/git`, `.gitconfig`,
    `.gitignore_global`, `.gitattributes`, `.profile`, `.bashrc`, `.bash_profile`, `.bash_login`,
    `.inputrc`, `.editorconfig`, `.docker/buildx`; then every `allow_read` path. (Not listed, so
    unreadable: `.ssh`, `.aws`, `.azure`, `.config/gcloud`, `.config/gh`, `.kube`, `.gnupg`,
    `.docker/config.json`, `.git-credentials`, `.netrc`, `.pypirc`, `.npmrc`, `.codex`,
    `.local/share/keyrings`, browser profiles, and every other file under `$HOME`.)
  - `default_profile` and `default_profile_net` on non-macOS return
    `LandlockConfined { write: default_allowed(root), read: read_allowed(<$HOME if set and non-empty>, allow_read), block_tcp: !network }`
    (`default_profile` is the `network == true` case).
  - `landlock_pre_exec` gains the read list: new signature
    `landlock_pre_exec(cmd, write: &[PathBuf], read: Option<&[PathBuf]>, block_tcp: bool)`. With
    `read: None`, exactly today's behaviour. With `Some(read)`, the ruleset also handles
    `AccessFs::from_read(ABI::V5)`; `write` paths get `from_all` (read and write) and `read` paths
    get `from_read`. The no-allocation rule inside `pre_exec` still holds.
- `shell.rs`: `Profile::LandlockConfined { write, read, block_tcp }` launches like the other Landlock
  variants, calling `landlock_pre_exec(&mut cmd, write, Some(read), *block_tcp)`; the existing
  variants pass `None`. On non-Linux it is the same `Unsupported` error as the others.
- `report.rs`, `sandbox_kind`: `LandlockConfined` → `"landlock"`, or `"landlock-offline"` when
  `block_tcp`.
- `task_runner.rs`: for `LandlockConfined`, replace the "restricts writes only; credential reads are
  not blocked" message with
  `blyte: on Linux, commands can read only system directories, the workspace, tool caches and --allow-read paths`,
  plus, when `block_tcp`, the existing TCP-only network note.
- Update the module doc of `sandbox.rs`: on Linux, reads are now an allowlist.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_linux_read_allowlist.rs`:

```rust
//! On Linux, commands read only system directories, the workspace, tool caches and grants:
//! Landlock cannot deny inside an allowed tree, but it can allow-list reads (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::read_allowed;

#[test]
fn the_allowlist_has_the_system_and_tool_dirs_but_no_credential_stores() {
    let home = Path::new("/home/someone");
    let r = read_allowed(Some(home), &[PathBuf::from("/data/granted")]);
    for p in ["/usr", "/etc", "/tmp", "/proc"] {
        assert!(r.contains(&PathBuf::from(p)), "{p} missing");
    }
    for p in [".cargo", ".rustup", ".cache", ".config/git", ".gitconfig", ".docker/buildx"] {
        assert!(r.contains(&home.join(p)), "{p} missing");
    }
    for p in [".ssh", ".aws", ".config/gh", ".codex", ".docker/config.json", ".npmrc", ".local", ".config"] {
        assert!(!r.contains(&home.join(p)), "{p} must not be readable");
    }
    assert!(!r.contains(&home.to_path_buf()), "the home directory itself is not granted");
    assert_eq!(r.last(), Some(&PathBuf::from("/data/granted")));
    assert!(read_allowed(None, &[]).iter().all(|p| !p.starts_with("/home")));
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;
    use std::time::Duration;

    use blyte_agent::sandbox::{available, read_allowed, Profile};
    use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

    fn landlock_or_skip() -> bool {
        if available() {
            return true;
        }
        assert!(std::env::var_os("CI").is_none(), "Landlock unavailable on CI");
        false
    }

    fn confined(ws: &std::path::Path, grants: &[PathBuf]) -> Profile {
        Profile::LandlockConfined {
            write: vec![ws.to_path_buf(), "/dev".into(), "/tmp".into()],
            read: read_allowed(None, grants),
            block_tcp: false,
        }
    }

    #[test]
    fn a_confined_command_reads_the_workspace_and_system_but_not_other_home_files() {
        if !landlock_or_skip() {
            return;
        }
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = LocalShell::sandboxed(confined(ws.path(), &[]));
        let ok = sh
            .exec("echo hi > f.txt && cat f.txt && ls /usr/bin >/dev/null", ws.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let denied = sh
            .exec(&format!("cat '{}'", secret.path().join("key").display()), ws.path(), Duration::from_secs(30))
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
        assert!(!denied.merged().contains("s3cret"));
    }

    #[test]
    fn an_allow_read_grant_makes_a_home_path_readable() {
        if !landlock_or_skip() {
            return;
        }
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = LocalShell::sandboxed(confined(ws.path(), &[secret.path().to_path_buf()]));
        let out = sh
            .exec(&format!("cat '{}'", secret.path().join("key").display()), ws.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(out.exit_code, 0, "{}", out.merged());
        assert!(out.merged().contains("s3cret"));
    }

    #[test]
    fn a_confined_persistent_session_works_and_is_confined() {
        if !landlock_or_skip() {
            return;
        }
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = PersistentShell::local_sandboxed(ws.path().to_path_buf(), confined(ws.path(), &[]));
        let ok = sh.exec("echo hi > g.txt && cat g.txt", ws.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let denied = sh
            .exec(&format!("cat '{}'", secret.path().join("key").display()), ws.path(), Duration::from_secs(30))
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
    }
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_linux_read_allowlist`: compile error.

- [ ] **Step 3: Implement** the additions above, and make the two named test changes.

- [ ] **Step 4: Verify.** Both checks from Global Constraints pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): on Linux, commands can read only system directories, the workspace, tool caches and grants"
```
