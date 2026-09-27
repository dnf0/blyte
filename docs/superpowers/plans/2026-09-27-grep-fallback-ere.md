# `grep` Fallback Uses Extended Regex — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** in a container without `rg`, blyte's `grep` tool runs `grep -E` (extended regular
expressions), so patterns with `|`, `+`, `?`, `(…)` or `{n}` match as they do with `rg` and
in-process search.

**Why (2026-09-27):** re-running FrontierHarness's `sanitize-git-repo`, blyte called its `grep`
tool with `(AKIA|ghp_|github_pat_|hf_|AWS_ACCESS_KEY|AWS_SECRET|api_key|token)` and got
"no matches": the container has no `rg`, and the fallback ran `grep -rnI -e …`, basic regular
expressions, where `|` and `( )` are literal. The secrets were there. blyte stopped trusting the
tool, spent 98 turns in `bash`, and missed a file again. Every container without `rg` (most task
images) had this.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**.
- The `rg` command and in-process search are unchanged.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `grep -E` in the fallback

**Files:** `crates/blyte-agent/src/tools.rs` (`grep_command`),
`crates/blyte-agent/tests/test_grep_fallback_ere.rs` (new).

**What to build:** in `grep_command`'s non-`rg` branch, `grep -rnI` becomes `grep -rnIE`. Nothing
else changes.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_grep_fallback_ere.rs`:

```rust
//! Without rg, the grep tool's fallback uses extended regex: basic `grep` read `|` and `( )`
//! literally, so `(AKIA|hf_)…` found nothing in a container that held the secrets (2026-09-27).

use std::time::Duration;

use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::tools::grep_command;

#[test]
fn the_fallback_asks_for_extended_regex() {
    let cmd = grep_command(false, "(a|b)+", ".", "");
    assert!(cmd.starts_with("grep -rnIE "), "{cmd}");
}

#[test]
fn alternation_and_repetition_match_through_the_fallback() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f.txt"), "nothing\ntoken hf_abc123 here\n").unwrap();
    let cmd = grep_command(false, "(AKIA|hf_)[a-z0-9]+", ".", "");
    let out = LocalShell.exec(&cmd, d.path(), Duration::from_secs(30)).unwrap();
    assert!(out.stdout.contains("hf_abc123"), "{}", out.merged());
}

#[test]
fn the_rg_command_is_unchanged() {
    assert!(grep_command(true, "x", ".", "").starts_with("rg --line-number"));
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_grep_fallback_ere`: the first two tests fail.

- [ ] **Step 3: Implement** the one-flag change.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "fix(agent): the grep tool's fallback uses extended regex, so alternation matches without rg"
```
