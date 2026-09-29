# Withhold the AWS Profile Variables — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-29):** blyte's sandbox denies reads of `~/.aws`, but the credential filter lets
`AWS_PROFILE` through, because no segment of the name looks like a credential. boto then looks for
a named profile in a file it cannot read and fails, even under moto. In all three premise-lookup
runs of 2026-09-29 (GS1–GS3), blyte worked around it by adding `os.environ.pop("AWS_PROFILE")` to the
repository's `conftest.py`: an out-of-scope edit. agy has no sandbox, so its runs passed without it.
A profile name selects credentials, so withhold it with them; `--pass-env AWS_PROFILE` (with
`--allow-read ~/.aws`) opts back in.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: withhold `AWS_PROFILE` and `AWS_DEFAULT_PROFILE`

**Files:** `crates/blyte-agent/src/shell.rs`, `crates/blyte-agent/tests/test_env_credential_filter.rs`
(one new test appended; existing tests unchanged), `docs/security.md`.

- [ ] **Step 1: test.** Append to `crates/blyte-agent/tests/test_env_credential_filter.rs`:

```rust
#[test]
fn aws_profile_names_are_credential_like() {
    // A profile selects credentials in ~/.aws, which the sandbox denies: passing the name makes
    // boto fail on a profile it cannot read (premise-lookup GS1-GS3, 2026-09-29).
    for name in ["AWS_PROFILE", "AWS_DEFAULT_PROFILE", "aws_profile"] {
        assert!(credential_like(name, "dev"), "{name}");
    }
    for name in [
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "PROFILE",
        "MY_AWS_PROFILE_DIR",
    ] {
        assert!(!credential_like(name, "x"), "{name}");
    }
}
```

  Run: `cargo test -p blyte-agent --test test_env_credential_filter`
  Expected: the new test fails (`AWS_PROFILE` is not credential-like yet); the others pass.

- [ ] **Step 2: implement.** In `crates/blyte-agent/src/shell.rs`, add after `CREDENTIAL_SEGMENTS`:

```rust
/// Whole names that select credentials without looking like one: the AWS profile variables pick a
/// profile in ~/.aws, which the sandbox denies (a passed profile makes boto fail on it).
const CREDENTIAL_NAMES: &[&str] = &["AWS_PROFILE", "AWS_DEFAULT_PROFILE"];
```

  and in `credential_like`, directly after `let upper = name.to_ascii_uppercase();`:

```rust
    if CREDENTIAL_NAMES.contains(&upper.as_str()) {
        return true;
    }
```

  Also change `credential_like`'s doc comment to begin: "True when the upper-cased name is one of
  `CREDENTIAL_NAMES`, or, split on `_`, has a credential segment, or ..." (keep the rest).

  Run: `cargo test -p blyte-agent --test test_env_credential_filter`
  Expected: all pass.

- [ ] **Step 3: docs.** In `docs/security.md`, in the table row that begins
  `| **Environment**: variables that look like credentials are withheld from commands`, change
  `variables that look like credentials are withheld from commands` to
  `variables that look like credentials, and the AWS profile variables, are withheld from commands`
  (the rest of the row unchanged).

- [ ] **Step 4: run the checks and commit.**

  Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
  Expected: all pass.

```bash
git add crates/blyte-agent/src/shell.rs crates/blyte-agent/tests/test_env_credential_filter.rs docs/security.md
git commit --no-gpg-sign -m "fix(agent): withhold AWS_PROFILE and AWS_DEFAULT_PROFILE with the credentials"
```
