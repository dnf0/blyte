# Remove the Commit Footer — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** Delete the `[after commit]` footer added in #84. A `bash` result is again exactly the
command's exit code and output.

**Why:** #84 was an experiment with a stated rule: delete it unless later runs make fewer
git-inspection calls (`git status` / `log` / `diff` / `show`) than the 4–6 per run before it. The two
runs since made 4 (S10) and 6 (E5, which ran `git log -1 --stat` right after a commit whose result
already carried that information). It did not work, so it goes: every feature earns its place or is
deleted.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass, except `crates/blyte-agent/tests/test_commit_footer.rs`, which tests the
  removed feature and is deleted.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Remove the footer

**Files:** `crates/blyte-agent/src/tools.rs`; delete `crates/blyte-agent/tests/test_commit_footer.rs`;
new test `crates/blyte-agent/tests/test_no_commit_footer.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_no_commit_footer.rs`:

```rust
//! A `bash` result is the command's exit code and output, nothing more: the #84 commit footer did
//! not reduce git-inspection turns (4 and 6 calls in the next two runs, against 4-6 before) and
//! was removed.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

#[test]
fn a_commit_result_is_just_the_command_output() {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             echo 0 > a.txt && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    let out = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "echo 1 > a.txt && git commit -qam 'feat: change'"}),
    )
    .content;
    assert_eq!(out.trim_end(), "exit 0", "{out}");
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_no_commit_footer`. Expected: fails (the footer is appended).

- [ ] **Step 3: Implement.** In `crates/blyte-agent/src/tools.rs`, restore the `"bash"` arm of
  `execute_unbounded` to what it was before #84 (run the command with `shell.exec_user`, return
  `exit N` and the distilled output, `is_error` when non-zero), and delete `runs_git_commit` and
  anything else that only served the footer. Delete `crates/blyte-agent/tests/test_commit_footer.rs`.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates/
git commit --no-gpg-sign -m "refactor(agent): remove the commit footer (it did not reduce git-inspection turns)"
```
