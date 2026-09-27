# Skill: Default On, Parallel by Default — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not write to the real `~/.claude`: tests use a temporary `HOME`. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `blyte skill install` makes blyte Claude Code's default implementer without a flag
(`--no-default` opts out), and the skill and the CLAUDE.md block make **parallel dispatch** the
default way of working.

**Why (2026-09-27):** the user's direction: parallel dispatch is how development speed is
maximised, and most developers do not know to ask for it, so it must be what installing the skill
gives them.

**Tech Stack:** Rust, clap.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**, with one named exception: in
  `crates/blyte-cli/tests/test_skill_default.rs`, the test
  `install_without_default_leaves_claude_md_alone` becomes
  `install_with_no_default_leaves_claude_md_alone` and runs `["skill", "install", "--no-default"]`
  (nothing else in it changes).
- `--default` stays accepted (hidden, a no-op), so existing scripts and tests keep working.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Default on, `--no-default`, parallel-first wording

**Files:** `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/src/skill.rs`, `crates/blyte-cli/assets/blyte-implementer/SKILL.md`,
`crates/blyte-cli/tests/test_skill_default.rs` (the named change), and
`crates/blyte-cli/tests/test_skill_default_on.rs` (new).

**What to build:**

- `SkillAction::Install` gains `#[arg(long)] no_default: bool` ("Install the skill without making
  blyte the default in CLAUDE.md"). The existing `default` flag becomes `#[arg(long, hide = true)]`
  and has no effect. `install` writes the CLAUDE.md block unless `no_default` is set.
- `DEFAULT_BLOCK` becomes exactly:
  ```text
  <!-- blyte:default -->
  ## Implementation: delegate to blyte, in parallel

  For code changes in a git repository that has a check command (tests, lint), plan and review
  the work yourself and delegate the implementation to blyte with the `blyte-implementer` skill.
  Split the work into independent tasks and dispatch them in parallel (`blyte dispatch`, one
  worktree each); run tasks one after another only when one depends on another. Keep for
  yourself: design decisions, ambiguous requirements, one-line edits, and anything with no check.
  <!-- /blyte:default -->
  ```
- In `crates/blyte-cli/assets/blyte-implementer/SKILL.md`, make exactly these replacements:
  1. The `description:` line becomes:
     `description: Use by default whenever you would otherwise write the code yourself for a change in a git repository that has a check command (tests, lint): plan the change, split it into independent tasks and dispatch them to blyte in parallel (`blyte dispatch`, one worktree each, on a cheaper model, with the check as the gate), then verify every result yourself and merge. Skip it for design discussions, ambiguous requirements, one-line edits, and anything with no check.`
  2. The heading `## 1. Plan, and split for parallel work` and the paragraph under it up to (not
     including) the bullet list become:
     ```markdown
     ## 1. Plan for parallel work (the default)

     Parallel is the default: it is how this workflow saves time. Break the change into tasks of
     one commit each, arranged so that as many as possible touch disjoint files, then sort them:
     ```
  3. After the bullet list of section 1, add the paragraph:
     ```markdown
     Dispatch every independent task at once. Run tasks one after another only when one needs
     another's result. A plan with a single task is fine for a single change; do not split work
     artificially just to run it in parallel.
     ```
- `main.rs`: `install` prints `updated <CLAUDE.md path>` whenever the block was written.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-cli/tests/test_skill_default_on.rs`:

```rust
//! Installing the skill makes blyte the default, with parallel dispatch, without a flag: most
//! developers do not know to ask for either (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{DEFAULT_BLOCK, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn plain_install_writes_the_default_block() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let md = std::fs::read_to_string(home.path().join(".claude/CLAUDE.md")).unwrap();
    assert!(md.contains(DEFAULT_BLOCK), "{md}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("updated"));
}

#[test]
fn the_block_and_the_skill_make_parallel_the_default() {
    assert!(DEFAULT_BLOCK.contains("dispatch them in parallel"), "{DEFAULT_BLOCK}");
    assert!(SKILL_MD.contains("## 1. Plan for parallel work (the default)"));
    assert!(SKILL_MD.contains("Dispatch every independent task at once."));
    assert!(SKILL_MD.contains("dispatch them to blyte in parallel"));
}

#[test]
fn the_old_default_flag_is_still_accepted() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(blyte(cwd.path(), home.path(), &["skill", "install", "--default"]).status.code(), Some(0));
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_skill_default_on`: the assertions fail.

- [ ] **Step 3: Implement** the changes above, and make the one named change to `test_skill_default.rs`.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): installing the skill makes blyte the default, with parallel dispatch (--no-default opts out)"
```
