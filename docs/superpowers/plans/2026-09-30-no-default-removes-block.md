# `--no-default` Removes an Earlier Default Block — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-30):** a colleague asked whether blyte can be switched on and off. It can
(`blyte skill install`, `install --no-default`, `uninstall`), but going from on to "only when asked"
with `blyte skill install --no-default` left the default block in `~/.claude/CLAUDE.md`, so Claude
Code kept delegating by default until an `uninstall` first. `--no-default` should remove the block.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: `install --no-default` removes the default block

**Files:** `crates/blyte-cli/src/skill.rs`, `crates/blyte-cli/tests/test_skill_default.rs` (one test appended).

- [ ] **Step 1: test.** Append to `crates/blyte-cli/tests/test_skill_default.rs` (it uses the file's `blyte` helper and `DEFAULT_BLOCK`):

```rust
#[test]
fn install_with_no_default_removes_an_earlier_default_block() {
    // Switching from "on" to "only when asked" must not need an uninstall first (2026-09-30).
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let md = home.path().join(".claude/CLAUDE.md");
    std::fs::create_dir_all(md.parent().unwrap()).unwrap();
    std::fs::write(&md, "# My rules\n").unwrap();
    assert_eq!(
        blyte(cwd.path(), home.path(), &["skill", "install"])
            .status
            .code(),
        Some(0)
    );
    assert!(std::fs::read_to_string(&md)
        .unwrap()
        .contains(DEFAULT_BLOCK));

    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--no-default"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "# My rules\n");
    assert!(home
        .path()
        .join(".claude/skills/blyte-implementer/SKILL.md")
        .exists());
}
```

  Run: `cargo test -p blyte --test test_skill_default`. Expected: the new test fails (the block is still there).

- [ ] **Step 2: fix `install_for` in `crates/blyte-cli/src/skill.rs`.** Replace

```rust
    if !no_default {
        let md_path = instructions_path(harness, project)?;
        if let Some(parent) = md_path.parent() {
```

  with

```rust
    let md_path = instructions_path(harness, project)?;
    if !no_default {
        if let Some(parent) = md_path.parent() {
```

  and replace the end of that `if` block,

```rust
        let updated = set_default_block(&existing);
        std::fs::write(&md_path, updated)
            .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
        written.push(md_path);
    }
```

  with

```rust
        let updated = set_default_block(&existing);
        std::fs::write(&md_path, updated)
            .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
        written.push(md_path);
    } else if md_path.exists() {
        // Switching to "only when asked" drops the default block an earlier install wrote, so it
        // does not need an uninstall first (2026-09-30).
        let existing = std::fs::read_to_string(&md_path)
            .map_err(|e| format!("failed to read {}: {e}", md_path.display()))?;
        let updated = remove_default_block(&existing);
        if updated != existing {
            std::fs::write(&md_path, updated)
                .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
            written.push(md_path);
        }
    }
```

  Run: `cargo test -p blyte --test test_skill_default`. Expected: all pass.

- [ ] **Step 3: run the checks and commit.**

```bash
git add crates/blyte-cli/src/skill.rs crates/blyte-cli/tests/test_skill_default.rs
git commit --no-gpg-sign -m "fix(cli): skill install --no-default removes an earlier default block"
```
