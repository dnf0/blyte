# Task 1 Brief: `superstate-syntax` Engine Extensions

## Context & Objective
In `repos/superstate`, extend `crates/superstate-syntax` with language detection, symbol source slicing, outline formatting, and symbol lookup.

## Working Directory & Branch
- Repo: `/Users/danielfisher/repos/superstate`
- Branch: `feat/ast-symbol-inspection`

## Files to Touch
- Create: `crates/superstate-syntax/src/inspect.rs`
- Modify: `crates/superstate-syntax/src/lib.rs` (re-export `inspect::*`)
- Create Test: `crates/superstate-syntax/tests/test_inspect.rs`

## Exact Signatures Required
```rust
use std::path::Path;
use crate::symbols::{SupportedLanguage, SymbolInfo, SyntaxError};

pub fn detect_language(path: &Path) -> Option<SupportedLanguage>;
pub fn extract_symbols_from_file(path: &Path) -> Result<(SupportedLanguage, String, Vec<SymbolInfo>), SyntaxError>;
pub fn extract_symbol_source<'a>(source: &'a str, symbol: &SymbolInfo) -> &'a str;
pub fn format_outline(symbols: &[SymbolInfo]) -> String;
pub fn find_symbol<'a>(symbols: &'a [SymbolInfo], name: &str) -> Option<&'a SymbolInfo>;
```

## Details & Edge Cases
1. `detect_language`:
   - `.rs` -> `SupportedLanguage::Rust`
   - `.py`, `.pyx` -> `SupportedLanguage::Python`
   - `.ts`, `.tsx`, `.js`, `.jsx` -> `SupportedLanguage::TypeScript`
   - `.go` -> `SupportedLanguage::Go`
   - Others -> `None`
2. `extract_symbols_from_file`:
   - Checks if file exists. If not, return `SyntaxError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, ...))`
   - Detects language. If None, return `SyntaxError::UnsupportedLanguage(...)` (extend `SyntaxError` enum in `symbols.rs` or `inspect.rs` if needed).
   - Reads file content to string.
   - Calls `extract_symbols(&content, lang)` and returns `(lang, content, symbols)`.
3. `extract_symbol_source`:
   - `symbol.start_line` and `symbol.end_line` are 1-indexed.
   - Slices the lines corresponding to `[start_line..=end_line]`.
   - Handles boundary clamping gracefully if line numbers exceed file line count.
4. `format_outline`:
   - Produces a string where each symbol is formatted:
     `- [{kind:?}] {name} [lines {start_line}-{end_line}]`
5. `find_symbol`:
   - Case-insensitive search on `symbol.name`. Returns first match.

## TDD Instructions
1. Write failing test in `crates/superstate-syntax/tests/test_inspect.rs`.
2. Run `cargo test -p superstate-syntax --test test_inspect` to verify it fails.
3. Implement `inspect.rs` and re-export in `lib.rs`.
4. Run `cargo test -p superstate-syntax --test test_inspect` to verify it passes.
5. Run `cargo clippy -p superstate-syntax --all-targets -- -D warnings` and `cargo test -p superstate-syntax`.
6. Commit with `feat(syntax): add language detection, symbol slicing, and outline formatting` (`--no-gpg-sign`).
7. Write summary report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-1-syntax-report.md`.
