# Linux Write Sandbox (Landlock) — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not run Docker. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** On Linux, blyte's local shell refuses writes outside the same allow-list the macOS sandbox
uses (workspace, git common dir, temp dirs, tool caches, `/dev`), using the kernel's Landlock LSM.
No new binary, no root, no user namespaces. `--sandbox=off` turns it off as on macOS.

**Why (2026-09-27):** the write sandbox (#91) and read-deny (#95) exist only on macOS
(`sandbox-exec`); on Linux blyte's commands can write anywhere the user can. Landlock is
unprivileged, enabled by default on Ubuntu kernels, and is the only option that needs neither setuid
helpers nor user namespaces (Ubuntu 24.04 restricts those, which breaks bubblewrap).

**Scope limit (state it in docs):** Landlock can only *allow* beneath paths, so it cannot carve
`~/.ssh` out of a readable home. On Linux the sandbox restricts **writes only**; the credential
read-deny stays macOS-only (the environment filter from #96 applies on both).

**Tech Stack:** Rust, `landlock = "0.4.7"` and `libc = "0.2"` (Linux-only dependencies).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified** (including `test_sandbox.rs` and `test_sandbox_read_deny.rs`,
  which call `LocalShell::sandboxed(<String>)`).
- macOS behaviour must not change: same SBPL profile, same `sandbox-exec` launch.
- Checks, all must pass:
  - `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
  - Linux cross-check (this machine is macOS; this is the only way to compile the Linux code here):
    `CC_aarch64_unknown_linux_gnu=zigcc-aarch64-linux cargo clippy --target aarch64-unknown-linux-gnu --workspace --all-targets -- -D warnings`
    (the target and the `zigcc-aarch64-linux` wrapper are already installed; CI runs the Linux tests.)
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `Profile` enum and a Landlock launcher

**What to build** (names are part of the contract; the tests below check them):

`crates/blyte-agent/Cargo.toml`:
```toml
[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4.7"
libc = "0.2"
```

`crates/blyte-agent/src/sandbox.rs`:
- `pub enum Profile { Seatbelt(String), Landlock(Vec<PathBuf>) }` (derive `Debug, Clone, PartialEq, Eq`).
  `Seatbelt` is an SBPL profile for `sandbox-exec`; `Landlock` is the list of write-allowed paths.
- `impl From<String> for Profile` → `Profile::Seatbelt` (so existing `LocalShell::sandboxed(profile(..))` calls compile unchanged).
- `available()`: on Linux, true when the kernel supports Landlock:
  `unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<libc::c_void>(), 0usize, 1u32) } >= 1`
  (flag 1 is `LANDLOCK_CREATE_RULESET_VERSION`; the call returns the ABI version). macOS unchanged; other OSes false.
- `default_allowed(root)`: additionally push `/tmp`, `/var/tmp` and `/dev` (on macOS `/tmp` is a symlink
  to `/private/tmp`, already allowed, so this is harmless there). Paths that do not exist are fine.
- `pub fn default_profile(root: &Path, allow_read: &[PathBuf]) -> Profile`: on macOS
  `Profile::Seatbelt(profile_with(&default_allowed(root), &read_denied(home), allow_read))` exactly as
  `task_runner.rs` builds it today (with the same empty-deny-list fallback when `$HOME` is unset or empty);
  on every other OS `Profile::Landlock(default_allowed(root))` (`allow_read` unused there).
- Linux only: `pub(crate) fn landlock_pre_exec(cmd: &mut std::process::Command, allowed: &[PathBuf]) -> std::io::Result<()>`:
  1. In the parent, build the ruleset with the crate's defaults (best-effort compatibility):
     `Ruleset::default().handle_access(AccessFs::from_write(ABI::V5))?.create()?.add_rules(path_beneath_rules(allowed, AccessFs::from_write(ABI::V5)))?`
     (map `RulesetError` to `io::Error::other`). `path_beneath_rules` skips paths that cannot be opened
     and narrows rights for files such as `/dev/null`.
  2. Move the `RulesetCreated` into `Mutex<Option<RulesetCreated>>` and register
     `unsafe { cmd.pre_exec(move || ...) }` that takes it out and calls `restrict_self()`. **Inside
     `pre_exec` do not allocate** (the child is a fork of a multi-threaded process): on error return
     `Err(std::io::Error::from_raw_os_error(libc::EPERM))`, never a formatted error. Document this in a
     `// SAFETY:` comment.

`crates/blyte-agent/src/shell.rs`:
- `LocalShell.profile` becomes `Option<Profile>`; `LocalShell::sandboxed(profile: impl Into<Profile>)`.
  `Launcher::Local.profile` becomes `Option<Profile>`; `PersistentShell::local_sandboxed(root, profile: impl Into<Profile>)`
  and `PersistentShell::sandboxed` likewise.
- Where a sandboxed command is built today (`LocalShell::exec` and `Launcher::command`):
  - `Profile::Seatbelt(p)`: unchanged (`/usr/bin/sandbox-exec -p p bash ...`).
  - `Profile::Landlock(paths)`: plain `bash ...` as for the unsandboxed case, then on Linux
    `sandbox::landlock_pre_exec(&mut cmd, paths)`; if that returns an error, the command fails with
    that error (do not silently run unsandboxed). On non-Linux, a `Landlock` profile is an error
    (`io::ErrorKind::Unsupported`).
- `Launcher::command` currently returns `Command`; change it to `std::io::Result<Command>` if needed
  and propagate the error to wherever the session is spawned (a spawn failure already falls back to
  `fallback()`, which builds a `LocalShell` with the same profile, which will fail the same way;
  that is acceptable: an error, not an unsandboxed run).

`crates/blyte-cli/src/task_runner.rs`: replace the inline profile construction with
`blyte_agent::sandbox::default_profile(&config.root, &config.allow_read)`. When the profile is
`Landlock`, print once to stderr:
`blyte: on Linux the sandbox restricts writes only; credential reads are not blocked`.

Update the module doc of `sandbox.rs` to describe both platforms and the Linux scope limit.

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_sandbox_linux.rs`:

```rust
//! On Linux blyte's shell refuses writes outside the allow-list via Landlock: the macOS sandbox
//! (#91) left Linux runs able to write anywhere the user can (2026-09-27).

use std::path::PathBuf;

use blyte_agent::sandbox::{default_profile, Profile};

#[test]
fn a_string_profile_is_a_seatbelt_profile() {
    let p: Profile = String::from("(version 1)").into();
    assert_eq!(p, Profile::Seatbelt("(version 1)".into()));
}

#[test]
fn the_default_profile_matches_the_platform() {
    let d = tempfile::tempdir().unwrap();
    let p = default_profile(d.path(), &[]);
    if cfg!(target_os = "macos") {
        assert!(matches!(p, Profile::Seatbelt(ref s) if s.starts_with("(version 1)")), "{p:?}");
    } else {
        match p {
            Profile::Landlock(paths) => {
                assert!(paths.contains(&d.path().to_path_buf()), "{paths:?}");
                assert!(paths.contains(&PathBuf::from("/dev")), "{paths:?}");
            }
            other => panic!("expected Landlock, got {other:?}"),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::time::Duration;

    use blyte_agent::sandbox::{available, Profile};
    use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

    fn landlock_or_skip() -> bool {
        if available() {
            return true;
        }
        // CI (GitHub's Ubuntu runners) must have Landlock, or these tests prove nothing.
        assert!(std::env::var_os("CI").is_none(), "Landlock unavailable on CI");
        false
    }

    #[test]
    fn a_landlocked_command_can_write_inside_and_not_outside() {
        if !landlock_or_skip() {
            return;
        }
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        let sh = LocalShell::sandboxed(Profile::Landlock(vec![
            inside.path().to_path_buf(),
            "/dev".into(),
        ]));
        let ok = sh
            .exec("echo hi > ok.txt && echo hi > /dev/null", inside.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target = outside.path().join("no.txt");
        let denied = sh
            .exec(&format!("echo hi > '{}'", target.display()), inside.path(), Duration::from_secs(30))
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
        assert!(!target.exists());
    }

    #[test]
    fn a_landlocked_persistent_session_is_confined_too() {
        if !landlock_or_skip() {
            return;
        }
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        let sh = PersistentShell::local_sandboxed(
            inside.path().to_path_buf(),
            Profile::Landlock(vec![inside.path().to_path_buf(), "/dev".into(), "/tmp".into()]),
        );
        let ok = sh.exec("echo hi > ok.txt", inside.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target = outside.path().join("no.txt");
        let denied = sh
            .exec(&format!("echo hi > '{}'", target.display()), inside.path(), Duration::from_secs(30))
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
        assert!(!target.exists());
    }
}
```

(`outside` is created under `$HOME`, not the temp dir, because `/tmp` is on the allow-list. If
`PersistentShell`'s session needs a temp directory elsewhere, add that path to the test's list.)

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_sandbox_linux`: compile error (`Profile`, `default_profile` missing).

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run both checks from Global Constraints. All pass. (The `linux` module
cannot run here; the cross-check must compile it cleanly.)

- [ ] **Step 5: Commit.**

```bash
git add -A crates Cargo.lock
git commit --no-gpg-sign -m "feat(agent): on Linux blyte's shell runs under Landlock, refusing writes outside the workspace"
```
