# Task 1 Brief: `superstate-core` Guard Engine & Evaluation Logic

## Context & Base Commit
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`
- **Base Commit:** `e1add33`
- **Spec:** `docs/superpowers/specs/2026-09-20-file-read-guard-design.md`
- **Plan:** `docs/superpowers/plans/2026-09-20-file-read-guard.md`

## What Needs to be Built
Implement the file read guard evaluation engine in `crates/superstate-core/src/guard.rs` and re-export in `lib.rs`.

### Types & Interface
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum GuardDecision {
    Allow,
    Deny { reason: String },
}

#[derive(Debug, Clone)]
pub struct FileReadGuardConfig {
    pub threshold_lines: usize,
}

impl Default for FileReadGuardConfig {
    fn default() -> Self {
        Self { threshold_lines: 200 }
    }
}

pub fn evaluate_file_read_guard(
    path: &std::path::Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
    config: &FileReadGuardConfig,
) -> GuardDecision;
```

### Evaluation Logic Rules
1. **File non-existent:** If `!path.exists()`, return `GuardDecision::Allow` (lets tool fail naturally).
2. **Directory:** If `path.is_dir()`, return `GuardDecision::Allow`.
3. **Binary detection:** If file content contains null bytes (`0x00`) in the first 8KB, return:
   `GuardDecision::Deny { reason: format!("[superstate-guard] Binary file read blocked: {}", path.display()) }`.
4. **Line count:** Read text. Count lines (`lines().count()`).
5. **Permitted reads:**
   - If total lines $\le$ `config.threshold_lines`: return `GuardDecision::Allow`.
   - If `start_line` and `end_line` are specified, and `end_line.saturating_sub(start_line) <= config.threshold_lines`: return `GuardDecision::Allow`.
6. **Denied reads (unbounded or range > threshold):**
   - Attempt AST symbol outline using `superstate_syntax::inspect::format_outline(path)`:
     - If successful and outline is non-empty:
       ```rust
       format!(
           "[superstate-guard] Full file read blocked ({} lines exceeds {}-line budget).\n\nAST Symbol Outline:\n{}\n\nRe-read using bounded StartLine and EndLine (<= {} lines), or inspect specific symbols with 'superstate inspect'.",
           total_lines, config.threshold_lines, outline, config.threshold_lines
       )
       ```
     - If language is unsupported or outline empty:
       Format a structural summary with total line count, the first 30 lines, and guidance:
       ```rust
       format!(
           "[superstate-guard] Full file read blocked ({} lines exceeds {}-line budget).\n\nFile preview (first 30 lines):\n{}\n...\nRe-read using bounded StartLine and EndLine (<= {} lines).",
           total_lines, config.threshold_lines, preview, config.threshold_lines
       )
       ```

## Steps (Follow TDD Strictly)
1. Write failing integration tests in `crates/superstate-core/tests/test_guard.rs`.
2. Run `cargo test -p superstate-core --test test_guard` to verify failure.
3. Implement `crates/superstate-core/src/guard.rs` and update `crates/superstate-core/src/lib.rs`.
4. Run `cargo test -p superstate-core --test test_guard` to verify passing.
5. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
6. Commit using `git commit --no-gpg-sign -m "feat(core): implement file read guard engine and ast shunting"`.
7. Write your report to `docs/superpowers/plans/task-1-report.md`.
