# Skill: blyte by Default, and Parallel Dispatch — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not write to the real `~/.claude`: tests use a temporary `HOME`. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** (1) Claude Code uses blyte for implementation work **without being asked**, when the
user opts in with `blyte skill install --default`. (2) The skill teaches **parallel dispatch**:
split work into independent tasks, dispatch them at once, review each as it lands, merge in order.

**Why (2026-09-27):** in an eval, Claude used the skill only because the prompt said "use blyte",
and the skill's one line on parallel work left Claude running tasks one after another. The
orchestration this repository uses runs independent tasks in parallel worktrees; six concurrent
`blyte dispatch` runs were checked by hand and do not race in git.

**Tech Stack:** Rust, clap.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `--default`, `blyte skill uninstall`, and the new skill text

**Files:** `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/src/skill.rs`, `crates/blyte-cli/assets/blyte-implementer/SKILL.md` (replace
with the text below, verbatim), `crates/blyte-cli/tests/test_skill_default.rs` (new).

**What to build:**

- `SkillAction::Install` gains `#[arg(long)] default: bool` ("Also tell Claude Code to delegate
  implementation to blyte by default, in CLAUDE.md"). New variant
  `SkillAction::Uninstall { #[arg(long)] project: bool }` ("Remove the skill and the CLAUDE.md
  block").
- In `skill.rs`:
  - `pub const DEFAULT_BLOCK: &str`, exactly:
    ```text
    <!-- blyte:default -->
    ## Implementation: delegate to blyte

    For code changes in a git repository that has a check command (tests, lint), plan and review
    the work yourself and delegate the implementation to blyte with the `blyte-implementer` skill
    (`blyte dispatch`), running independent tasks in parallel. Keep for yourself: design decisions,
    ambiguous requirements, one-line edits, and anything with no check.
    <!-- /blyte:default -->
    ```
  - `pub fn claude_md_path(project: bool) -> Result<PathBuf, String>`: `<repo root>/CLAUDE.md`
    with `project` (same root lookup as `skill_dir`), else `$HOME/.claude/CLAUDE.md`.
  - `pub fn set_default_block(text: &str) -> String`: if `text` contains a block from
    `<!-- blyte:default -->` to `<!-- /blyte:default -->` (inclusive), replace it with
    `DEFAULT_BLOCK`; otherwise append it, separated from existing content by one blank line (no
    leading blank line when `text` is empty). The result ends with exactly one `\n`.
  - `pub fn remove_default_block(text: &str) -> String`: remove the block, the newline that ends it,
    and the blank line before it (if any), so that `remove_default_block(&set_default_block(t)) == t`
    for any `t` that ends with a newline; text without a block is returned unchanged.
  - `install(project, force, default)`: as today, then with `default` write
    `set_default_block(<existing CLAUDE.md or "">)` to `claude_md_path(project)` (creating the
    directory). Returns the skill path.
  - `pub fn uninstall(project: bool) -> Result<Vec<PathBuf>, String>`: delete the skill directory's
    `SKILL.md` (and the directory if then empty) and remove the block from `CLAUDE.md` if present
    (leave the file, even if it becomes empty). Returns the paths it changed; missing files are not
    an error.
- `main.rs`: `install` prints `installed <skill path>`, plus `updated <CLAUDE.md path>` when
  `--default`, then the existing hint line. `uninstall` prints `removed <path>` per path (or
  `nothing to remove`), exit 0; errors to stderr, exit 1.

**New `crates/blyte-cli/assets/blyte-implementer/SKILL.md`**, verbatim:

````markdown
---
name: blyte-implementer
description: Use by default whenever you would otherwise write the code yourself for a change in a git repository that has a check command (tests, lint): plan the change, split it into precise tasks, dispatch each to blyte (`blyte dispatch`, in its own worktree, on a cheaper model, with the check as its gate), independent tasks in parallel, then verify every result yourself and merge. Skip it for design discussions, ambiguous requirements, one-line edits, and anything with no check.
---

# blyte as your implementer

You plan and review; blyte implements, on a cheaper model, and its gate (the repository's check
command) decides whether the work is done. Your job is to make each task unambiguous, run
independent tasks at the same time, and verify every result, not to trust it.

## When to delegate

Delegate when a careful engineer could do the task from its description alone, without asking
anything, and the repository has a check that would catch a wrong result. Keep for yourself:
design decisions, ambiguous requirements, one-line edits, anything with no check, and anything
touching credentials, deployment or other repositories.

## 1. Plan, and split for parallel work

Break the change into tasks of one commit each. Then sort them:

- **Independent tasks** touch disjoint files and do not depend on each other: they run in parallel.
- **Dependent tasks** need an earlier task's result: they run after it, based on its branch
  (`--base wip/T1`).
- **Coupled tasks** (task 2 cannot pass its checks without task 1) are one task.

## 2. Write each task

One task per Markdown file. blyte sees only this file and the repository, nothing from your
conversation. Include:

- **Goal**, in one or two sentences, and **why**.
- **Files** to create or change.
- **What to build**: names, signatures and behaviour that are part of the contract; exact code
  where it matters.
- **Tests** that prove it, written out, and the command that runs them.
- **Constraints**, e.g. "existing tests pass unmodified".
- **Siblings**: for a task running in parallel, name the other tasks and say they are **not** in
  this worktree ("T2 adds `slugify` separately; do not add it here"), so it does not "fix" their
  absence.
- **The exact commit message.**

Before dispatching, check that the tests fail on the current code and can pass with the intended
change.

## 3. Dispatch

From the repository's main checkout. Each dispatch takes minutes.

```bash
blyte dispatch T1 /abs/path/task-T1.md --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

- Each line of the gate is a step; all must pass. Put the slow full test run last.
- The implementer model's API key must be in the environment; blyte never prints it.

**Parallel tasks in an interactive session:** send every independent `blyte dispatch` as its own
background command in the same message. You are notified as each one exits; review each as it
lands instead of waiting for all of them.

**Parallel tasks in a one-shot or headless session** (for example `claude -p`): background commands
die when the session ends, so start them together and wait in the foreground:

```bash
mkdir -p .wt
blyte dispatch T1 /abs/task-T1.md --gate '...' > .wt/T1.json &
blyte dispatch T2 /abs/task-T2.md --gate '...' > .wt/T2.json &
wait
```

Each dispatch prints one JSON object: `outcome` (`verified`, `no_changes`, `gate_failed`,
`budget_exhausted`, `error`), `exit` (0 only for `verified`), `commits`, `changed_files`,
`final_message` (blyte's own report), `gate_failures`, `turns`, `cost_usd`, `seconds`, and the
`log` and `report` paths. `no_changes` means the gate passed on an untouched tree: not done.

## 4. Verify each result. Do not trust the summary

```bash
blyte review T1 --diff
```

1. **Scope**: only the files the task names changed, in the commit(s) you asked for, with the exact message.
2. **Fidelity**: code you gave verbatim landed verbatim; read everything else in full.
3. **Checks yourself**: run the gate in the worktree (`.wt/T1`). A passing gate is necessary, not sufficient.
4. **Deviations**: every deviation in `final_message` is justified, and there are none it did not mention.
5. **Out-of-scope effects**: skim the log (`.wt/T1.log`) for commands that touched anything outside the worktree.

If it falls short: fix a trivial issue yourself, or write a follow-up task saying exactly what to
change and dispatch it (`blyte dispatch T1b follow-up.md --base wip/T1 --gate ...`). If the task was
wrong, fix the task, not the result.

## 5. Merge

Merge verified tasks one at a time:

```bash
git merge --ff-only wip/T1
git worktree remove .wt/T1 && git branch -d wip/T1
```

When an earlier merge has moved the branch, a parallel task no longer fast-forwards: rebase it
first (`git -C .wt/T2 rebase <branch>`), re-run its gate in the worktree, then merge it. After the
last merge, run the full check once more on the merged branch. Push or open a pull request only if
the user asks.

## Rules

- One task per dispatch; never hand over a whole plan.
- blyte never pushes, merges or reads secrets. If its log shows otherwise, stop and tell the user.
- Report to the user what **you verified**, not what blyte claimed.
````

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-cli/tests/test_skill_default.rs`:

```rust
//! `blyte skill install --default` makes delegating to blyte Claude Code's default through a
//! marked CLAUDE.md block; `uninstall` removes it. The skill teaches parallel dispatch
//! (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{remove_default_block, set_default_block, DEFAULT_BLOCK, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn the_block_is_appended_once_and_replaced_in_place() {
    assert_eq!(set_default_block(""), format!("{DEFAULT_BLOCK}\n"));
    let once = set_default_block("# Me\n\nMy rules.\n");
    assert_eq!(once, format!("# Me\n\nMy rules.\n\n{DEFAULT_BLOCK}\n"));
    assert_eq!(set_default_block(&once), once, "idempotent");
    let stale = "# Me\n\n<!-- blyte:default -->\nold text\n<!-- /blyte:default -->\n\n## After\n";
    let fixed = set_default_block(stale);
    assert!(fixed.contains(DEFAULT_BLOCK) && !fixed.contains("old text"), "{fixed}");
    assert!(fixed.contains("## After"), "{fixed}");
    assert_eq!(remove_default_block(&once), "# Me\n\nMy rules.\n");
    assert_eq!(remove_default_block("# Me\n"), "# Me\n");
}

#[test]
fn install_default_writes_claude_md_and_uninstall_removes_both() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let md = home.path().join(".claude/CLAUDE.md");
    std::fs::create_dir_all(md.parent().unwrap()).unwrap();
    std::fs::write(&md, "# My rules\n").unwrap();

    let out = blyte(cwd.path(), home.path(), &["skill", "install", "--default"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("updated"));
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(text.starts_with("# My rules\n") && text.contains(DEFAULT_BLOCK), "{text}");

    let out = blyte(cwd.path(), home.path(), &["skill", "uninstall"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "# My rules\n");
    assert!(!home.path().join(".claude/skills/blyte-implementer/SKILL.md").exists());
    let again = blyte(cwd.path(), home.path(), &["skill", "uninstall"]);
    assert_eq!(again.status.code(), Some(0));
}

#[test]
fn install_without_default_leaves_claude_md_alone() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(blyte(cwd.path(), home.path(), &["skill", "install"]).status.code(), Some(0));
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
}

#[test]
fn the_skill_is_a_default_and_teaches_parallel_dispatch() {
    assert!(SKILL_MD.contains("description: Use by default"), "description must trigger by default");
    for s in ["in parallel", "Siblings", "wait", "rebase", "--base wip/T1"] {
        assert!(SKILL_MD.contains(s), "{s} missing");
    }
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_skill_default`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): blyte skill install --default makes blyte Claude Code's default implementer; the skill teaches parallel dispatch"
```
