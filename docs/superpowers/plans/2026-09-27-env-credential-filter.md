# Credential-Looking Environment Filter — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not print the values of any real environment variable. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** Commands blyte runs on the host no longer inherit environment variables that look like
credentials (`GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY`, `SSH_AUTH_SOCK`, a `DATABASE_URL` with a
password, …), unless the user keeps one with `--pass-env NAME`. blyte says once, on stderr, which
names it withheld (names only, never values).

**Why (2026-09-27):** the shell scrub removes blyte's own provider keys (`HARNESS_ENV`) but passes
everything else, so `env` shows the model any token the user has exported. A strict allowlist would
break tools (every toolchain reads its own variables); filtering by name shape removes the
credentials and keeps the rest.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**. `HARNESS_ENV` and the `BLYTE_*` removal stay as they are.
- Container commands are unchanged (they never inherit the host environment).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Withhold credential-looking variables, `--pass-env` keeps one

**What to build** (names are part of the contract; the tests below check them):

`crates/blyte-agent/src/shell.rs`:
- `pub fn credential_like(name: &str, value: &str) -> bool`, true when either:
  - the upper-cased name, split on `_`, has a segment in
    `TOKEN, SECRET, SECRETS, PASSWORD, PASSWD, PASS, KEY, CREDENTIAL, CREDENTIALS, AUTH, COOKIE, SESSION`
    (so `GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `SSH_AUTH_SOCK`, `NPM_TOKEN`
    match, while `TOKENIZERS_PARALLELISM`, `KEYCHAIN_PATH`-style whole words that merely contain a
    listed word, `SECURITYSESSIONID` and `PATH` do not); or
  - the value contains a URL with inline credentials: `://`, then a user part, `:`, a non-empty
    password part, then `@`, with no `/` or whitespace in between (for example
    `postgres://u:p@host/db`). No regex crate: scan by hand.
- `pub fn set_pass_env(names: Vec<String>)`: process-wide (a `std::sync::OnceLock<Vec<String>>`);
  names listed here are never withheld by the credential filter (they are still removed if they are
  in `HARNESS_ENV` or start with `BLYTE_`). Calling it a second time is a no-op.
- `pub fn withheld_env() -> Vec<String>`: the sorted names of the current process's variables that
  the credential filter withholds (credential-like, not passed, and not already in `HARNESS_ENV`
  or `BLYTE_*`).
- `scrub` additionally removes every name in `withheld_env()`.

`crates/blyte-cli/src/cli.rs`: `--pass-env NAME`, repeatable
(`#[arg(long = "pass-env")] pub pass_env: Vec<String>`), doc: "Pass this environment variable to
commands even though it looks like a credential (repeatable)". Carry it through `TaskConfig` as
`pass_env: Vec<String>` (set it to `Vec::new()` wherever a `TaskConfig` is built by hand).

`crates/blyte-cli/src/task_runner.rs`: before the shell is built for a local (non-container) run,
call `blyte_agent::shell::set_pass_env(config.pass_env.clone())`, then, if
`withheld_env()` is non-empty, print once to stderr:
`blyte: withholding from commands: A, B (keep one with --pass-env NAME)`.

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_env_credential_filter.rs`:

```rust
//! Commands blyte runs do not see credential-looking variables: the old scrub removed only blyte's
//! own provider keys, so `env` showed the model any token the user had exported (2026-09-27).

use std::time::Duration;

use blyte_agent::shell::{credential_like, set_pass_env, withheld_env, LocalShell, Shell};

#[test]
fn names_with_a_credential_segment_are_credential_like() {
    for name in [
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_ACCESS_KEY_ID",
        "AWS_SESSION_TOKEN",
        "SSH_AUTH_SOCK",
        "NPM_TOKEN",
        "DB_PASSWORD",
        "hf_token",
    ] {
        assert!(credential_like(name, "x"), "{name}");
    }
    for name in [
        "PATH",
        "HOME",
        "TOKENIZERS_PARALLELISM",
        "SECURITYSESSIONID",
        "CARGO_HOME",
        "KEYBOARD_LAYOUT",
        "AWS_REGION",
    ] {
        assert!(!credential_like(name, "x"), "{name}");
    }
}

#[test]
fn a_url_with_an_inline_password_is_credential_like() {
    assert!(credential_like("DATABASE_URL", "postgres://u:p@host:5432/db"));
    assert!(credential_like("X", "see https://user:secret@example.com/path"));
    assert!(!credential_like("DATABASE_URL", "postgres://host:5432/db"));
    assert!(!credential_like("X", "https://example.com/a:b@c"));
    assert!(!credential_like("X", "https://user@example.com/"));
}

// One test touches the process environment and the process-wide pass list, so it is the only
// test in this binary that does, and it runs as a single #[test].
#[test]
fn withheld_variables_are_removed_from_commands_and_passed_ones_are_kept() {
    std::env::set_var("ZZ_BLYTE_TEST_TOKEN", "t0ken-value");
    std::env::set_var("ZZ_BLYTE_TEST_PASS_TOKEN", "kept-value");
    std::env::set_var("ZZ_BLYTE_TEST_PLAIN", "plain-value");
    set_pass_env(vec!["ZZ_BLYTE_TEST_PASS_TOKEN".into()]);

    let withheld = withheld_env();
    assert!(withheld.contains(&"ZZ_BLYTE_TEST_TOKEN".to_string()), "{withheld:?}");
    assert!(!withheld.contains(&"ZZ_BLYTE_TEST_PASS_TOKEN".to_string()), "{withheld:?}");
    assert!(!withheld.iter().any(|n| n == "GEMINI_API_KEY"), "HARNESS_ENV names are not listed");

    let dir = tempfile::tempdir().unwrap();
    let out = LocalShell
        .exec("env", dir.path(), Duration::from_secs(30))
        .unwrap();
    let env = out.merged();
    assert!(!env.contains("t0ken-value"), "withheld variable leaked");
    assert!(env.contains("ZZ_BLYTE_TEST_PASS_TOKEN=kept-value"));
    assert!(env.contains("ZZ_BLYTE_TEST_PLAIN=plain-value"));
}
```

Also add a CLI test (in `crates/blyte-cli/tests/test_task_runner.rs` or a new
`crates/blyte-cli/tests/test_pass_env_cli.rs`) that `Cli::try_parse_from` with
`--pass-env A --pass-env B` yields `pass_env == ["A", "B"]` and that `TaskConfig::from(&cli)` carries it.

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_env_credential_filter`: compile error.

- [ ] **Step 3: Implement** the additions above. If `LocalShell` is a `const`/unit value in this
codebase use it as the test does; if it is not, adapt only the construction in this new test.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): commands no longer inherit credential-looking environment variables (--pass-env keeps one)"
```
