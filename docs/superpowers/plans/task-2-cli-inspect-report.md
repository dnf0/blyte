# Task 2 Report: `superstate-cli` `inspect` Command Implementation & Installation

**Status:** Completed  
**Commit SHA:** `bdbc8a5`  
**Branch:** `feat/ast-symbol-inspection`  
**Repository:** `/Users/danielfisher/repos/superstate`  
**Date:** 2026-09-20  

---

## Executive Summary

Task 2 implemented the `superstate inspect` CLI command in `crates/superstate-cli`, integrating the AST inspection capabilities provided by `superstate-syntax::inspect` into the primary unified `superstate` binary as specified in `docs/superpowers/plans/task-2-cli-inspect-brief.md`.

The new command equips agents and developers with precise, bounded source inspection tools to retrieve outlines, symbol definitions, or line slices without flooding LLM context windows with entire files.

### Key Features Delivered

1. **`superstate inspect <PATH> --lines <start:end>`**:
   - Universal line-slicing capability operating on any plain-text file (including languages without AST parser support).
   - Strict 1-indexed validation checking bounds (`start > 0`, `end > 0`, `start <= end`, valid integer formatting).
   - Formatted output with contextual header and 1-indexed line prefixing:
     ```text
     === Lines {start}-{end} in {path} ===
     {line_no}: {line_content}
     ```
   - Returns exit code 1 with explicit diagnostic error on invalid syntax:
     `Error: invalid line range '<arg>'. Expected format 'start:end' (e.g. 10:50)`

2. **`superstate inspect <PATH> --symbol <name>`**:
   - Case-insensitive symbol lookup leveraging `superstate_syntax::inspect::find_symbol`.
   - Formatted source body extraction with line numbers:
     ```text
     === Symbol: {symbol.name} [lines {symbol.start_line}-{symbol.end_line}] in {path} ===
     {line_no}: {line_content}
     ```
   - Structured JSON output when paired with `--json` emitting the serialized `SymbolInfo` model.
   - Informative stderr diagnostics when symbol is absent, listing all available symbols extracted from the file:
     `Error: symbol '{name}' not found in {path}. Available symbols: [name1, name2, ...]`

3. **Default / Outline Mode (`superstate inspect <PATH> [--outline]`)**:
   - Default mode when neither `--symbol` nor `--lines` is supplied.
   - Text output displaying detected language and compact symbol signatures:
     ```text
     === Outline: {path} ({lang:?}) ===
     - [{kind:?}] {name} [lines {start_line}-{end_line}]
     ```
   - JSON array output of `Vec<SymbolInfo>` when `--json` flag is provided.

4. **Error Handling & File Checks**:
   - Non-existent files: `Error: file not found at {path}` (exit code 1).
   - Unsupported file extensions during AST extraction: `Error: unsupported language for AST parsing in '{path}'. Use --lines <start:end>` (exit code 1).

5. **Binary Reinstallation & Verification**:
   - Recompiled and installed release binary to `~/.cargo/bin/superstate` with `--locked --force`.
   - Verified `--help` and live command execution across multiple inspection modes.

---

## TDD Execution

### 1. Red Phase
Created `crates/superstate-cli/tests/test_cli_inspect.rs` with 10 integration tests:
- `test_cli_inspect_help`: Validates flags `--symbol`, `--outline`, `--lines`, `--json`.
- `test_cli_inspect_file_not_found`: Confirms exit code 1 and error message on non-existent paths.
- `test_cli_inspect_unsupported_language_ast`: Confirms graceful error advising `--lines` on unsupported file types.
- `test_cli_inspect_lines_valid`: Confirms line slice extraction and headers on text files.
- `test_cli_inspect_lines_invalid_syntax`: Tests parsing errors (non-integer, 0-indexed, inverted range).
- `test_cli_inspect_outline_default_text`: Verifies default outline generation without explicit flags.
- `test_cli_inspect_outline_json`: Verifies JSON array serialization of symbols in outline mode.
- `test_cli_inspect_symbol_found_text`: Verifies single-symbol extraction with line numbers.
- `test_cli_inspect_symbol_found_json`: Verifies structured JSON serialization of matching symbol.
- `test_cli_inspect_symbol_not_found`: Verifies stderr reporting and available symbols listing on missing symbol.

Ran `cargo test -p superstate --test test_cli_inspect`:
```text
running 10 tests
test test_cli_inspect_help ... FAILED
test test_cli_inspect_file_not_found ... FAILED
test test_cli_inspect_unsupported_language_ast ... FAILED
test test_cli_inspect_outline_default_text ... FAILED
test test_cli_inspect_symbol_found_json ... FAILED
test test_cli_inspect_lines_invalid_syntax ... FAILED
test test_cli_inspect_lines_valid ... FAILED
test test_cli_inspect_outline_json ... FAILED
test test_cli_inspect_symbol_not_found ... FAILED
test test_cli_inspect_symbol_found_text ... FAILED

test result: FAILED. 0 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s
```

### 2. Green Phase
- Implemented `crates/superstate-cli/src/commands/inspect.rs` containing `InspectArgs`, `parse_line_range`, `inspect_lines`, `inspect_symbol`, `inspect_outline`, and `cmd_inspect`.
- Registered `pub mod inspect;` in `crates/superstate-cli/src/commands/mod.rs`.
- Added `Commands::Inspect(commands::inspect::InspectArgs)` and its dispatch branch in `crates/superstate-cli/src/main.rs`.

Ran `cargo test -p superstate --test test_cli_inspect`:
```text
running 10 tests
test test_cli_inspect_help ... ok
test test_cli_inspect_file_not_found ... ok
test test_cli_inspect_unsupported_language_ast ... ok
test test_cli_inspect_lines_valid ... ok
test test_cli_inspect_symbol_found_text ... ok
test test_cli_inspect_symbol_found_json ... ok
test test_cli_inspect_outline_default_text ... ok
test test_cli_inspect_outline_json ... ok
test test_cli_inspect_symbol_not_found ... ok
test test_cli_inspect_lines_invalid_syntax ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.42s
```

### 3. Verification & Cleanliness Checks
- Ran `cargo clippy --all-targets -- -D warnings`: Passed cleanly with 0 warnings.
- Ran `cargo test --all`: All unit, integration, and doc tests across all 7 workspace crates passed.

---

## Installation & System Verification

Reinstalled the release binary via `cargo install --path crates/superstate-cli --locked --force`:
```text
Replacing /Users/danielfisher/.cargo/bin/state
Replacing /Users/danielfisher/.cargo/bin/superstate
```

Verified `~/.cargo/bin/superstate inspect --help`:
```text
Inspect AST symbols, outlines, and code slices

Usage: superstate inspect [OPTIONS] <PATH>

Arguments:
  <PATH>  Path to target source file to inspect

Options:
      --symbol <SYMBOL>  Extract and print only the body of a specific function, class, or struct
      --outline          Print compact symbol outline (signatures and line numbers)
      --lines <LINES>    Print bounded line range in format 'start:end' (1-indexed)
      --json             Emit output in structured JSON format
  -h, --help             Print help
```

Verified real commands against source files in the repository:
1. `superstate inspect crates/superstate-cli/src/commands/inspect.rs --lines 1:5`: Sliced header lines correctly.
2. `superstate inspect crates/superstate-cli/src/commands/inspect.rs --outline`: Generated structured AST symbol outline.
3. `superstate inspect crates/superstate-cli/src/commands/inspect.rs --symbol parse_line_range`: Extracted function implementation with line numbers.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-cli/src/commands/inspect.rs` | Created | Implemented `cmd_inspect`, `InspectArgs`, line slicing, symbol extraction, and outline generation |
| `crates/superstate-cli/src/commands/mod.rs` | Modified | Registered `pub mod inspect;` |
| `crates/superstate-cli/src/main.rs` | Modified | Added `Inspect(commands::inspect::InspectArgs)` to `Commands` enum and dispatch |
| `crates/superstate-cli/tests/test_cli_inspect.rs` | Created | Added 10 integration tests covering all arguments, modes, error cases, and JSON serialization |

---

## Commit Details

- **Commit:** `bdbc8a5`
- **Message:** `feat(cli): add superstate inspect command for AST symbol extraction and slicing`
