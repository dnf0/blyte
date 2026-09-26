# State After a Commit — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** When a `bash` call runs `git commit` and exits 0, blyte appends the repository's resulting
state to that tool result: the new commit (hash, subject, files changed) and the working tree status
(`working tree clean`, or the `git status --short` lines). A failed commit, or any command without
`git commit`, is unchanged.

**Why (shadow runs S6–S8, 2026-09-26):** in every run the model follows a commit with `git log -1`
(or `--stat`) and `git status` as separate turns, 4–6 turns per run (about 12%), each re-sending the
whole prompt. Asking it to batch did not stop this. Handing it the state it goes looking for, in the
result it already gets, removes the reason for those turns. **This is an experiment:** if later runs
do not show fewer git-inspection calls, the feature is deleted.

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- The extra state is gathered with the same shell session the command ran in (`shell.exec_user`), so
  it reflects the directory the model committed from. Its output goes through the same bounding as
  the rest of the result.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Append the repository state after a successful commit

**Files:** `crates/blyte-agent/src/tools.rs`; new test `crates/blyte-agent/tests/test_commit_footer.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_commit_footer.rs`:

```rust
//! After a successful `git commit`, the result carries the new commit and the working tree state,
//! so the model has no reason to spend turns on `git log -1` and `git status`.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             echo 0 > a.txt && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    d
}

fn bash(d: &tempfile::TempDir, command: &str) -> String {
    execute(d.path(), &LocalShell, &LocalFs, "bash", &json!({ "command": command })).content
}

#[test]
fn a_successful_commit_reports_the_commit_and_a_clean_tree() {
    let d = repo();
    let out = bash(&d, "echo 1 > a.txt && git add -A && git commit -q -m 'feat: change a'");
    assert!(out.starts_with("exit 0"), "{out}");
    assert!(out.contains("[after commit]"), "{out}");
    assert!(out.contains("feat: change a"), "the new commit's subject: {out}");
    assert!(out.contains("a.txt"), "the files it changed: {out}");
    assert!(out.contains("working tree clean"), "{out}");
}

#[test]
fn leftover_changes_are_listed() {
    let d = repo();
    let out = bash(&d, "echo 1 > a.txt && echo x > b.txt && git add a.txt && git commit -q -m 'feat: a only'");
    assert!(out.contains("[after commit]"), "{out}");
    assert!(out.contains("?? b.txt"), "the untracked file is shown: {out}");
    assert!(!out.contains("working tree clean"), "{out}");
}

#[test]
fn a_failed_commit_gets_no_footer() {
    let d = repo();
    let out = bash(&d, "git commit -q -m 'nothing to commit'");
    assert!(!out.starts_with("exit 0"), "{out}");
    assert!(!out.contains("[after commit]"), "{out}");
}

#[test]
fn other_commands_get_no_footer() {
    let d = repo();
    let out = bash(&d, "git log --oneline -1");
    assert!(!out.contains("[after commit]"), "{out}");
    let out = bash(&d, "echo 'we will git commit later'");
    assert!(!out.contains("[after commit]"), "text that merely mentions a commit is not one: {out}");
}
```

  The last case means "runs `git commit`" should be judged from the command's shell words, not a
  substring: treat it as a commit when `git` is immediately followed by `commit` as separate words
  outside quotes (a simple scan is enough; `echo '... git commit ...'` must not match).

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_commit_footer`. Expected: the first two tests fail.

- [ ] **Step 3: Implement.** In the `"bash"` arm of `execute_unbounded` in
  `crates/blyte-agent/src/tools.rs`: when the command runs `git commit` (as above) and the exit code
  is 0, run `git log -1 --stat --format='%h %s' && git status --short` through `shell.exec_user`
  in the same `root`, and append to the result a line `[after commit]` followed by that output,
  with `working tree clean` in place of an empty status. Keep the helper that detects a commit small
  and give it a doc comment saying why the footer exists (the Why above, in two lines).

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-agent --test test_commit_footer --test test_tools --test test_bound_output`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): a successful git commit reports the new commit and the tree state"
```
