# Sandbox Read-Deny — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not read or print any real credential file. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** On macOS, blyte's sandboxed shell also refuses to **read** credential stores in the user's
home directory (SSH keys, cloud and CLI tokens, keychains, browser profiles) and any file named
`.env`, unless the user grants a path back with `--allow-read PATH`.

**Why (2026-09-27):** the write sandbox (#91) stops a command changing the host, but any command can
still `cat ~/.aws/sso/cache/*` or `~/.ssh/id_ed25519` and send it anywhere: the model sees tool
output, and bash has the network. Reading credentials is never part of an implementer's job.

**Tech Stack:** Rust, macOS Seatbelt (SBPL). In SBPL the **last matching rule wins**; this was
checked by hand: a `(deny file-read-data (subpath X))` followed by `(allow file-read-data (subpath X))`
allows X. Denying `~/Library/Keychains` does not break HTTPS (`curl`, `git ls-remote` over https).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**. Do not edit `tests/test_sandbox.rs`.
- `profile(allowed)` keeps its signature and its current output as a prefix (existing tests check it).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Deny credential reads, `--allow-read` grants

**What to build** (names are part of the contract; the tests below check them):

`crates/blyte-agent/src/sandbox.rs`:
- `pub fn read_denied(home: &Path) -> Vec<PathBuf>`: `home` joined with each of
  `.ssh`, `.aws`, `.azure`, `.config/gcloud`, `.config/gh`, `.kube`, `.gnupg`, `.docker/config.json`,
  `.git-credentials`, `.netrc`, `.pypirc`, `.cargo/credentials.toml`, `.cargo/credentials`, `.codex`,
  `Library/Keychains`, `Library/Cookies`, `Library/Safari`,
  `Library/Application Support/Google/Chrome`, `Library/Application Support/Firefox`,
  `Library/Application Support/Arc`, `Library/Application Support/BraveSoftware`,
  in that order.
- `pub fn profile_with(write: &[PathBuf], deny_read: &[PathBuf], allow_read: &[PathBuf]) -> String`:
  1. exactly `profile(write)`, then
  2. `(deny file-read-data (subpath "P"))` for each `deny_read` path, then
  3. `(deny file-read-data (regex #"/\.env$"))`, then
  4. `(allow file-read-data (subpath "P"))` for each `allow_read` path.
  Paths are canonicalised when possible and escaped exactly as `profile` does (share a helper).
  Because the last match wins, an `allow_read` grant overrides both the list and the `.env` rule.
- Update the module doc comment to mention reads.

`crates/blyte-cli/src/cli.rs`: `--allow-read PATH`, repeatable
(`#[arg(long = "allow-read")] pub allow_read: Vec<PathBuf>`), doc: "Let sandboxed commands read
this path despite the credential deny-list (repeatable)". Carry it through `TaskConfig` as
`allow_read: Vec<PathBuf>` (set it to `Vec::new()` wherever a `TaskConfig` is built by hand, e.g.
`repl.rs` and tests).

`crates/blyte-cli/src/task_runner.rs`: where the sandboxed shell is built, use
`profile_with(&default_allowed(&config.root), &read_denied(&home), &config.allow_read)` with `home`
from `$HOME` (if `$HOME` is unset or empty, pass an empty deny list).

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_sandbox_read_deny.rs`:

```rust
//! blyte's sandboxed shell refuses to read credential stores and `.env` files: the write sandbox
//! stopped host damage, but a command could still read ~/.aws or ~/.ssh and print it (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::{profile, profile_with, read_denied};

#[test]
fn the_deny_list_covers_the_usual_credential_stores() {
    let home = Path::new("/Users/someone");
    let denied = read_denied(home);
    for sub in [".ssh", ".aws", ".config/gh", ".codex", "Library/Keychains", ".docker/config.json"] {
        assert!(denied.contains(&home.join(sub)), "{sub} missing from {denied:?}");
    }
}

#[test]
fn the_read_rules_follow_the_write_profile_and_grants_come_last() {
    let write = [PathBuf::from("/nonexistent/ws")];
    let p = profile_with(
        &write,
        &[PathBuf::from("/nonexistent/secret")],
        &[PathBuf::from("/nonexistent/secret/ok")],
    );
    assert!(p.starts_with(&profile(&write)), "{p}");
    let deny = p.find(r#"(deny file-read-data (subpath "/nonexistent/secret"))"#).expect(&p);
    let env = p.find(r##"(deny file-read-data (regex #"/\.env$"))"##).expect(&p);
    let grant = p.find(r#"(allow file-read-data (subpath "/nonexistent/secret/ok"))"#).expect(&p);
    assert!(deny < env && env < grant, "{p}");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;
    use std::time::Duration;

    use blyte_agent::sandbox::profile_with;
    use blyte_agent::shell::{LocalShell, Shell};

    fn cat(sh: &LocalShell, file: &Path, cwd: &Path) -> Option<(i32, String)> {
        let out = sh
            .exec(&format!("cat '{}'", file.display()), cwd, Duration::from_secs(30))
            .unwrap();
        if out.exit_code == 71 && out.merged().contains("Operation not permitted") {
            // Nested sandboxing is not permitted here (e.g. already inside a sandbox).
            return None;
        }
        Some((out.exit_code, out.merged()))
    }

    #[test]
    fn a_denied_file_and_a_dot_env_cannot_be_read_but_other_files_can() {
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir().unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        std::fs::write(ws.path().join(".env"), "TOKEN=x").unwrap();
        std::fs::write(ws.path().join(".env.example"), "TOKEN=").unwrap();
        let sh = LocalShell::sandboxed(profile_with(
            &[ws.path().to_path_buf()],
            &[secret.path().to_path_buf()],
            &[],
        ));
        let Some((code, out)) = cat(&sh, &secret.path().join("key"), ws.path()) else {
            return;
        };
        assert_ne!(code, 0, "{out}");
        assert!(!out.contains("s3cret"), "{out}");
        let (code, out) = cat(&sh, &ws.path().join(".env"), ws.path()).unwrap();
        assert!(code != 0 && !out.contains("TOKEN=x"), "{out}");
        let (code, out) = cat(&sh, &ws.path().join(".env.example"), ws.path()).unwrap();
        assert_eq!(code, 0, "{out}");
    }

    #[test]
    fn an_allow_read_grant_overrides_the_deny_list() {
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir().unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = LocalShell::sandboxed(profile_with(
            &[ws.path().to_path_buf()],
            &[secret.path().to_path_buf()],
            &[secret.path().to_path_buf()],
        ));
        let Some((code, out)) = cat(&sh, &secret.path().join("key"), ws.path()) else {
            return;
        };
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("s3cret"), "{out}");
    }
}
```

Also add to `crates/blyte-cli/tests/test_task_runner.rs` (or a new `crates/blyte-cli/tests/test_allow_read_cli.rs`)
a test that `Cli::try_parse_from(["blyte", "--allow-read", "/a", "--allow-read", "/b", ...])`
yields `allow_read == [/a, /b]` and that `TaskConfig::from(&cli)` carries it.

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_sandbox_read_deny`: compile error (`profile_with`, `read_denied` missing).

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): the macOS sandbox refuses reads of credential stores and .env files (--allow-read grants)"
```
