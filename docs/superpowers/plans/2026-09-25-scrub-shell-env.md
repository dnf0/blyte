# Scrub the Harness's Own Environment from the Model's Shell Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Commands blyte runs on this machine no longer inherit blyte's own configuration or provider credentials. Provider tests stop depending on an ambient `BLYTE_MODEL`.

**Why (found by the first blyte dogfood run, 2026-09-25):** Local shells inherited blyte's whole environment.
1. **The model could read the API keys:** `env` in its shell showed `GEMINI_API_KEY`.
2. **`BLYTE_MODEL` leaked into the repository's tests:** with it set, 7 provider tests fail. The baseline check failed before any edit, so the gate could never pass, and the run spent about 20 turns investigating an environment problem.

Container runs were unaffected: `docker exec` does not pass the host environment through.

**Design:**
- One list in `shell.rs`, `HARNESS_ENV`, holding `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `ANTHROPIC_API_KEY`, `OLLAMA_API_KEY` and `OLLAMA_HOST`.
- A rule: every variable whose name starts with `BLYTE_` is also removed.
- Both are applied to every local process blyte starts for commands: the persistent session (`Launcher::Local`) and the process-per-command `LocalShell`.
- The container launcher needs nothing.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `fix/scrub-shell-env` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- Do not run Docker. `Cargo.lock` is gitignored.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass, **and must also pass with `BLYTE_MODEL=gemini-3.8-flash-high` exported** (Step 5).
- Existing tests must pass. The only permitted edits to existing tests are in Step 3.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Scrub the environment; make provider tests hermetic

**Files:** `crates/blyte-agent/src/shell.rs`, `crates/blyte-provider/tests/{test_ollama_wire.rs,test_gemini_wire.rs,test_anthropic_wire.rs,test_gemini_models.rs,test_registry.rs}` (Step 3 only), and a new test `crates/blyte-agent/tests/test_shell_env.rs`.

- [x] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_shell_env.rs`:

```rust
//! Commands blyte runs never see its own configuration or provider credentials. The model could
//! read GEMINI_API_KEY with `env`, and BLYTE_MODEL broke the repository's own tests.
#![cfg(unix)]

use std::time::Duration;

use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

const T: Duration = Duration::from_secs(30);

#[test]
fn credentials_and_blyte_configuration_do_not_reach_commands() {
    // One test owns these variables for this binary, so there is no race with other tests.
    std::env::set_var("GEMINI_API_KEY", "secret-not-real");
    std::env::set_var("ANTHROPIC_API_KEY", "secret-not-real");
    std::env::set_var("BLYTE_MODEL", "gemini-3.8-flash-high");
    std::env::set_var("BLYTE_ANYTHING_ELSE", "x");
    std::env::set_var("UNRELATED_USER_VAR", "kept");
    let probe = "echo ${GEMINI_API_KEY:-unset} ${ANTHROPIC_API_KEY:-unset} \
                 ${BLYTE_MODEL:-unset} ${BLYTE_ANYTHING_ELSE:-unset} ${UNRELATED_USER_VAR:-unset}";
    let expected = "unset unset unset unset kept\n";

    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    assert_eq!(LocalShell.exec(probe, &root, T).unwrap().stdout, expected, "LocalShell");
    let p = PersistentShell::local(root.clone());
    assert_eq!(p.exec(probe, &root, T).unwrap().stdout, expected, "harness session");
    assert_eq!(p.exec_user(probe, &root, T).unwrap().stdout, expected, "model's session");
}
```

- [x] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_shell_env`. Expected: FAIL (the probe prints the secret values).

- [x] **Step 3: Make the provider tests hermetic.** In each provider test file listed above, in every helper or test that calls a provider's `from_env()` **without** being a test about `BLYTE_MODEL` itself, add `std::env::remove_var("BLYTE_MODEL");` next to the existing `set_var` / `remove_var` calls, under the same `ENV_LOCK` guard the file already uses. Tests that are about `BLYTE_MODEL` (e.g. `from_env_honors_blyte_model`) must set it explicitly, and remove it again before they finish. Change nothing else in these files.

- [x] **Step 4: Scrub.** In `crates/blyte-agent/src/shell.rs`:
  1. Add, near the top (after the imports):

```rust
/// Variables the harness itself uses, never passed to the commands it runs: provider credentials
/// (the model could otherwise read them with `env`) and blyte's own configuration (a
/// `BLYTE_MODEL` in the environment broke the repository's provider tests). Every variable named
/// `BLYTE_*` is removed as well. Container commands never inherit the host environment.
pub const HARNESS_ENV: &[&str] = &[
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OLLAMA_API_KEY",
    "OLLAMA_HOST",
];

/// Remove the harness's own variables from a command about to run on this machine.
fn scrub(cmd: &mut Command) {
    for name in HARNESS_ENV {
        cmd.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("BLYTE_") {
            cmd.env_remove(&name);
        }
    }
}
```

  2. In `impl Shell for LocalShell`, `exec`: call `scrub(&mut cmd);` after building `cmd` and before `exec_command(cmd, timeout)`.
  3. In `Launcher::command`, in the `Launcher::Local { root }` arm, call `scrub(&mut c);` before returning `c`. Do not scrub the `Container` arm.

- [x] **Step 5: Run the tests.** Run: `cargo test -p blyte-agent --test test_shell_env --test test_persistent_shell --test test_session_protocol`. Expected: all pass. Then run the full check twice: once normally, and once as `BLYTE_MODEL=gemini-3.8-flash-high cargo test --workspace --all-targets`. Expected: 0 failed both times.

- [x] **Step 6: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [x] **Step 7: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "fix(agent): commands never inherit provider credentials or BLYTE_* configuration"
```
