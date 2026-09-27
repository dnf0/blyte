# Sandbox blyte's Shell on macOS — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** On macOS, every command blyte runs on the local host runs under `sandbox-exec` with a profile
that refuses file writes outside an allow-list: the workspace root, the git directory its commits
land in, temporary directories, `/dev`, and the usual developer tool caches under the home
directory. On by default for local runs; `--sandbox=off` turns it off. On other platforms, and for
container runs, nothing changes.

**Why (2026-09-26):** after a gate failure, blyte's model spent 50 minutes "freeing disk space" on
the host (`uv cache clean`, `find ~/.cache/uv … | xargs …`). The workspace boundary is now stated in
the prompt (#87), but a prompt is not enforcement: the model's shell runs with the user's full
permissions. `sandbox-exec` is present on current macOS and enforces a write boundary (checked on
macOS 26.6: a write inside the allowed directory succeeded, a write to `$HOME` failed with "Operation
not permitted"). Tool caches stay writable because cargo, uv and npm must write there, so this blocks
damage to other repositories, home files and system settings, not cache clean-ups.

**Tech Stack:** Rust 2021, macOS Seatbelt profiles (SBPL).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified, on macOS and on Linux (CI runs both).
- No new dependencies.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: The sandbox

**What to build** (names are part of the contract):

New module `crates/blyte-agent/src/sandbox.rs`, exported as `pub mod sandbox` from `lib.rs`:
- `pub fn profile(allowed: &[PathBuf]) -> String`: an SBPL profile that allows everything except
  file writes, then allows file writes under each allowed path. Exactly:
  `(version 1)(allow default)(deny file-write*)` followed by
  `(allow file-write* (subpath "<path>"))` for each path (paths canonicalised when they exist, and
  `"` / `\` escaped), plus `(allow file-write* (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))`.
- `pub fn default_allowed(root: &Path) -> Vec<PathBuf>`: the workspace root; the git common directory
  of `root` when `root` is in a git repository (from `git -C <root> rev-parse --path-format=absolute
  --git-common-dir`), since a worktree's commits are written there; `/private/tmp`, `/private/var/folders`
  and `$TMPDIR` if set; and these under `$HOME` when `$HOME` is set: `.cargo`, `.rustup`, `.cache`,
  `.npm`, `.local`, `Library/Caches`, `.gradle`, `.m2`, `go`.
- `pub fn available() -> bool`: true on macOS when `/usr/bin/sandbox-exec` exists; false elsewhere.

`crates/blyte-agent/src/shell.rs`: the local shells (`LocalShell` and the persistent local session)
gain an optional sandbox profile. When set, the command is started as
`/usr/bin/sandbox-exec -p <profile> bash …` instead of `bash …`, keeping the existing environment
scrub. Container shells are never sandboxed. Expose a way to set it, e.g. `LocalShell::sandboxed(profile)`
or a field; keep `LocalShell` itself (unit struct or `Default`) working unsandboxed as today so
existing tests are unchanged.

`crates/blyte-cli`: a `--sandbox <on|off>` flag (default `on`). For local (non-container) runs, when it
is `on` and `sandbox::available()`, build the profile from `sandbox::default_allowed(root)` and use a
sandboxed local shell for the run (tools and gate alike). When `on` but unavailable, run unsandboxed
and print one warning line. Record it in the run report as `sandboxed: bool`.

- [ ] **Step 1: Write the failing tests.**

Create `crates/blyte-agent/tests/test_sandbox.rs`:

```rust
//! On macOS blyte's shell refuses writes outside an allow-list: a model once spent 50 minutes
//! deleting a host cache after a gate failure, and a prompt is not enforcement.

use std::path::PathBuf;

use blyte_agent::sandbox::{default_allowed, profile};

#[test]
fn the_profile_denies_writes_then_allows_the_listed_paths() {
    let p = profile(&[PathBuf::from("/tmp/ws-\"q\"")]);
    assert!(p.starts_with("(version 1)(allow default)(deny file-write*)"), "{p}");
    assert!(p.contains(r#"(allow file-write* (subpath "/tmp/ws-\"q\""))"#), "{p}");
    assert!(p.contains(r#"(literal "/dev/null")"#), "{p}");
}

#[test]
fn the_default_allow_list_covers_the_workspace_and_its_git_directory() {
    let d = tempfile::tempdir().unwrap();
    std::process::Command::new("git").args(["init", "-q"]).current_dir(d.path()).status().unwrap();
    let allowed = default_allowed(d.path());
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(d.path());
    assert!(allowed.iter().any(|p| canon(p) == root), "{allowed:?}");
    assert!(allowed.iter().any(|p| canon(p) == root.join(".git")), "{allowed:?}");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;
    use std::time::Duration;

    use blyte_agent::sandbox::profile;
    use blyte_agent::shell::{LocalShell, Shell};

    #[test]
    fn a_sandboxed_command_can_write_inside_and_not_outside() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // A profile with only `inside` allowed (no temp dirs), so `outside` is refused.
        let sh = LocalShell::sandboxed(profile(&[inside.path().to_path_buf()]));
        let ok = sh
            .exec("echo hi > ok.txt", inside.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target: PathBuf = outside.path().join("no.txt");
        let denied = sh
            .exec(&format!("echo hi > {}", target.display()), inside.path(), Duration::from_secs(30))
            .unwrap();
        assert_ne!(denied.exit_code, 0);
        assert!(!target.exists(), "the write outside the allow-list must not happen");
    }
}
```

  If the local shell's constructor for a sandboxed instance ends up named differently, adapt only the
  construction line in the macOS test; keep every assertion.

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_sandbox`. Expected: compile error (no `sandbox` module).

- [ ] **Step 3: Implement** the module, the shell change and the CLI flag above.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_sandbox --test test_shell_env --test test_persistent_shell --test test_tools` and `cargo test -p blyte`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): on macOS blyte's shell runs sandboxed, refusing writes outside the workspace"
```
