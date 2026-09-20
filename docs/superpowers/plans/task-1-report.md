# Task 1 Report: `superstate-core` Guard Engine & Evaluation Logic

**Status:** Completed (DONE)  
**Commit SHA:** `57263284f50d55c0ea57a48bd400a1ef6dae4c34` (`5726328`)  
**Branch:** `feat/pre-tool-use-file-guard`  
**Repository:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`  
**Date:** 2026-09-20  

---

## Executive Summary

Task 1 implemented the core evaluation engine for deterministic `PreToolUse` file read guards in `crates/superstate-core/src/guard.rs` and re-exported its types in `crates/superstate-core/src/lib.rs`.

The guard inspects incoming file read targets and bounded slice parameters against a configurable threshold budget (default 200 lines):
1. Non-existent files and directories are allowed (`GuardDecision::Allow`), allowing native agent runtime error mechanisms to function normally.
2. Binary files (null bytes in first 8KB) are denied (`GuardDecision::Deny`), preventing LLM prompt state corruption.
3. Reads within the threshold budget ($\le 200$ lines total or bounded range $\le 200$ lines) are allowed.
4. Large unbounded reads or slices exceeding the threshold are denied and shunted:
   - For supported AST languages (Rust, Python, TypeScript, Go), tree-sitter extracts symbols and outputs a structured AST outline with line ranges and guidance to inspect specific symbols or re-read with bounded parameters.
   - For unsupported languages or files without extracted symbols, a structured fallback preview of the first 30 lines is returned alongside bounded re-read instructions.

---

## TDD Execution

### 1. Red Phase
Created comprehensive integration tests in `crates/superstate-core/tests/test_guard.rs` validating:
- `test_config_default_threshold`: Verifies default threshold is 200 lines.
- `test_serde_guard_decision`: Verifies JSON serialization and deserialization of `GuardDecision::Allow` (`{"decision":"allow"}`) and `GuardDecision::Deny` (`{"decision":"deny","reason":"..."}`).
- `test_non_existent_file_allows`: Verifies non-existent files return `Allow`.
- `test_directory_allows`: Verifies directory paths return `Allow`.
- `test_small_file_allows`: Verifies files $\le 200$ lines return `Allow`.
- `test_file_exactly_at_threshold_allows`: Verifies files at exactly 200 lines return `Allow`.
- `test_bounded_slice_under_threshold_allows`: Verifies explicit slice requests $\le 200$ lines return `Allow`.
- `test_large_file_unbounded_denies_with_ast_outline`: Verifies unbounded reads $> 200$ lines return `Deny` with an AST outline.
- `test_large_file_bounded_slice_exceeding_threshold_denies`: Verifies bounded requests $> 200$ lines return `Deny` with an AST outline.
- `test_binary_file_denies`: Verifies files containing null bytes in initial 8KB return `Deny` with binary file message.
- `test_unsupported_language_falls_back_to_preview`: Verifies unsupported extensions return `Deny` with first 30 lines preview.
- `test_empty_ast_outline_falls_back_to_preview`: Verifies files with no AST symbols (e.g. comment-only files) fall back to preview.

Ran `cargo test -p superstate-core --test test_guard` to verify RED failure:
```text
error[E0432]: unresolved import `superstate_core::guard`
 --> crates/superstate-core/tests/test_guard.rs:4:22
  |
4 | use superstate_core::guard::{evaluate_file_read_guard, FileReadGuardConfig, GuardDecision};
  |                      ^^^^^ could not find `guard` in `superstate_core`
error: could not compile `superstate-core` (test "test_guard") due to 1 previous error
```

### 2. Green Phase
1. Added `superstate-syntax = { path = "../superstate-syntax" }` to `crates/superstate-core/Cargo.toml`.
2. Implemented `GuardDecision`, `FileReadGuardConfig`, and `evaluate_file_read_guard` in `crates/superstate-core/src/guard.rs`:
   - Checks file existence and directory status.
   - Detects binary content via 8KB null byte scanning.
   - Computes line counts using `content.lines().count()`.
   - Permits reads $\le$ `threshold_lines` or slices with `end_line.saturating_sub(start_line) <= threshold_lines`.
   - Extracts AST symbols via `superstate_syntax::inspect::detect_language` and `superstate_syntax::symbols::extract_symbols`.
   - Formats AST outlines via `superstate_syntax::inspect::format_outline`.
   - Falls back to 30-line previews when unsupported or outline is empty.
3. Re-exported `guard` module and items in `crates/superstate-core/src/lib.rs`.

Ran `cargo test -p superstate-core --test test_guard`:
```text
running 12 tests
test test_config_default_threshold ... ok
test test_serde_guard_decision ... ok
test test_directory_allows ... ok
test test_non_existent_file_allows ... ok
test test_small_file_allows ... ok
test test_binary_file_denies ... ok
test test_large_file_bounded_slice_exceeding_threshold_denies ... ok
test test_unsupported_language_falls_back_to_preview ... ok
test test_file_exactly_at_threshold_allows ... ok
test test_empty_ast_outline_falls_back_to_preview ... ok
test test_large_file_unbounded_denies_with_ast_outline ... ok
test test_bounded_slice_under_threshold_allows ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

### 3. Static Analysis & Full Workspace Verification
- `cargo clippy --all-targets -- -D warnings`: Passed across entire workspace with 0 warnings.
- `cargo test --all`: All unit, integration, and doc tests across all crates passed.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-core/Cargo.toml` | Modified | Added `superstate-syntax` dependency |
| `crates/superstate-core/src/guard.rs` | Created | Implemented `evaluate_file_read_guard`, `FileReadGuardConfig`, and `GuardDecision` |
| `crates/superstate-core/src/lib.rs` | Modified | Declared and re-exported `guard` module |
| `crates/superstate-core/tests/test_guard.rs` | Created | Added 12 unit tests for guard evaluation and AST shunting |
| `docs/superpowers/plans/task-1-brief.md` | Modified | Updated task brief for Task 1 |

---

## Commit Details

- **Commit SHA:** `57263284f50d55c0ea57a48bd400a1ef6dae4c34` (`5726328`)
- **Message:** `feat(core): implement file read guard engine and ast shunting`
- **GPG Verification:** Skipped (`--no-gpg-sign`) per repository guidance.
