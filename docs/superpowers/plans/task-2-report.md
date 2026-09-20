# Task 2 Report: `superstate guard pre-tool-use` CLI Subcommand

**Status:** Completed (DONE)  
**Commit SHA:** `244d3c969b6702e17c63f9f4715e97b8e3fed8c9` (`244d3c9`)  
**Branch:** `feat/pre-tool-use-file-guard`  
**Repository:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`  
**Date:** 2026-09-20  

---

## Executive Summary

Task 2 implemented the CLI command `superstate guard pre-tool-use [--threshold-lines <N>]` in `crates/superstate-cli` to enable deterministic, hook-driven interception of file reading tool calls before payloads enter the LLM context.

The command operates via a robust stdin/stdout JSON protocol conforming to Antigravity and Claude Code `PreToolUse` hook contracts:
1. **Stdin Payload Processing & Fail-Open Resilience:**
   - Reads `stdin` to string.
   - If `stdin` is empty, missing, or malformed JSON, outputs `{"decision":"allow"}` and exits 0 to ensure agent runtimes never abort or deadlock due to hook evaluation errors.
2. **Tool Invocations & Normalization:**
   - Detects tool calls formatted across multiple standard agent conventions: nested `toolCall` / `tool_call` or top-level properties (`toolName`, `name`, `args`, `arguments`, `parameters`, `tool_input`).
   - Normalizes tool names to lowercase; permits non-file-reading tools (e.g. `run_command`, `write_to_file`) unconditionally with `{"decision":"allow"}`.
   - Evaluates targeted file-reading tools: `view_file`, `view`, `read_file`, `readfile`, `viewfile`.
3. **Argument Extraction & Range Normalization:**
   - Extracts target file path across common argument conventions (`AbsolutePath`, `path`, `file_path`, `filePath`, `TargetFile`, `target_file`, `file`). Permits empty or missing paths with `{"decision":"allow"}`.
   - Extracts line ranges from `StartLine`, `start_line`, `startLine`, `offset`, and `EndLine`, `end_line`, `endLine`, `limit` (supporting numeric and string values).
   - Normalizes head slice requests: when `end_line` is present but `start_line` is omitted, defaults `start_line` to `Some(1)`.
4. **Deterministic Guard Evaluation:**
   - Evaluates the read request against `superstate_core::guard::evaluate_file_read_guard` using configured or default line thresholds (default 200 lines).
   - Serializes outcome directly to stdout followed by newline (`{"decision":"allow"}` or `{"decision":"deny","reason":"..."}`) and exits code 0.

---

## TDD Execution

### 1. Red Phase
Created integration test suite in `crates/superstate-cli/tests/test_cli_guard.rs` covering:
- `test_empty_stdin_allows`: Empty input safely produces `{"decision":"allow"}` with exit code 0.
- `test_invalid_json_allows`: Malformed JSON input produces `{"decision":"allow"}` with exit code 0.
- `test_non_file_tool_call_allows`: Non-file tools (such as `run_command`) produce `{"decision":"allow"}`.
- `test_missing_or_empty_path_allows`: Tool calls with empty or absent path parameters produce `{"decision":"allow"}`.
- `test_non_existent_file_allows`: Reads on non-existent targets produce `{"decision":"allow"}` so runtime errors surface normally.
- `test_small_file_allows`: Reads on files $\le 200$ lines produce `{"decision":"allow"}`.
- `test_large_file_unbounded_denies_with_ast_outline`: Unbounded reads on files $> 200$ lines produce `{"decision":"deny"}` with AST symbol outline.
- `test_bounded_slice_under_threshold_allows`: Bounded slice requests $\le 200$ lines produce `{"decision":"allow"}`.
- `test_head_slice_with_only_end_line_allows`: Slice requests with only `EndLine: 50` properly normalize start line to 1 and produce `{"decision":"allow"}`.
- `test_bounded_slice_exceeding_threshold_denies`: Slices $> 200$ lines produce `{"decision":"deny"}`.
- `test_custom_threshold_lines_parameter`: Verifies `--threshold-lines 50` enforces smaller budget on an 80-line file.
- `test_flexible_payload_formats_and_tool_names`: Verifies Claude Code (`View`), snake_case (`tool_call`), and top-level schema compatibility.

Ran `cargo test -p superstate --test test_cli_guard` to confirm RED failure:
```text
failures:
    test_bounded_slice_exceeding_threshold_denies
    test_bounded_slice_under_threshold_allows
    test_custom_threshold_lines_parameter
    test_empty_stdin_allows
    test_flexible_payload_formats_and_tool_names
    test_head_slice_with_only_end_line_allows
    test_invalid_json_allows
    test_large_file_unbounded_denies_with_ast_outline
    test_missing_or_empty_path_allows
    test_non_existent_file_allows
    test_non_file_tool_call_allows
    test_small_file_allows

test result: FAILED. 0 passed; 12 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.48s
stderr: 'error: unrecognized subcommand 'guard'
```

### 2. Green Phase
1. Implemented `crates/superstate-cli/src/commands/guard.rs`:
   - Defined `GuardCommands` and `PreToolUseArgs`.
   - Implemented `cmd_pre_tool_use` with stdin streaming, fail-open error handling, argument extraction, head slice normalization, and `evaluate_file_read_guard` invocation.
   - Implemented `dispatch_guard`.
2. Registered `guard` module in `crates/superstate-cli/src/commands/mod.rs`.
3. Registered `Guard` subcommand in `crates/superstate-cli/src/main.rs` in `Commands` enum and dispatched in `main()`.

Ran `cargo test -p superstate --test test_cli_guard`:
```text
running 12 tests
test test_invalid_json_allows ... ok
test test_non_existent_file_allows ... ok
test test_empty_stdin_allows ... ok
test test_non_file_tool_call_allows ... ok
test test_bounded_slice_under_threshold_allows ... ok
test test_small_file_allows ... ok
test test_bounded_slice_exceeding_threshold_denies ... ok
test test_head_slice_with_only_end_line_allows ... ok
test test_missing_or_empty_path_allows ... ok
test test_large_file_unbounded_denies_with_ast_outline ... ok
test test_custom_threshold_lines_parameter ... ok
test test_flexible_payload_formats_and_tool_names ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s
```

### 3. Static Analysis & Full Workspace Verification
- `cargo clippy --all-targets -- -D warnings`: Finished with 0 warnings.
- `cargo test --all`: All unit, integration, and doc tests across the entire repository workspace passed.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-cli/src/commands/guard.rs` | Created | Implemented `superstate guard pre-tool-use` logic and clap arguments |
| `crates/superstate-cli/src/commands/mod.rs` | Modified | Registered `pub mod guard;` |
| `crates/superstate-cli/src/main.rs` | Modified | Added `Commands::Guard` variant and dispatch invocation |
| `crates/superstate-cli/tests/test_cli_guard.rs` | Created | Added 12 comprehensive integration tests for CLI guard subcommand |
| `docs/superpowers/plans/task-2-brief.md` | Modified | Updated task brief for Task 2 |

---

## Commit Details

- **Commit SHA:** `244d3c969b6702e17c63f9f4715e97b8e3fed8c9` (`244d3c9`)
- **Message:** `feat(cli): add superstate guard pre-tool-use CLI subcommand`
- **GPG Verification:** Skipped (`--no-gpg-sign`) per repository guidance.
