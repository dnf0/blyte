# Task 2 Report: CLI Subcommand `superstate guard pre-command` in `crates/superstate-cli`

**Status:** Completed (DONE)  
**Commit SHA:** `4c1afbb7fa8fa7652758bfd3215aa6f5193f41eb` (`4c1afbb`)  
**Branch:** `feat/pre-tool-use-command-guard`  
**Repository:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-command-guard`  
**Date:** 2026-09-20  

---

## Executive Summary

Task 2 implemented and wired the `pre-command` CLI subcommand under `superstate guard` in `crates/superstate-cli` and updated `state_cli.rs` to route the command for both `superstate` and the standalone `state` binaries.

The command intercepts command execution and task status checks before execution by reading tool call JSON payloads from `stdin` and emitting the PreToolUse decision JSON (`{"decision":"allow"}` or `{"decision":"deny","reason":"..."}`) to `stdout`.

Key features implemented:
1. **CLI Subcommand Structure:**
   - Added `GuardCommands::PreCommand(PreCommandArgs)` to `crates/superstate-cli/src/commands/guard.rs`.
   - Wired `PreCommandArgs` with `--debounce-sec <u64>` (default: 30) and `--state-path <PathBuf>` (optional state history file).
   - Added `StateCommands::Guard` in `crates/superstate-cli/src/state_cli.rs` and wired dispatch to guarantee unified parity across both `superstate` and `state` binaries.
2. **Stdin/Stdout Hook Protocol & Fail-Open Behavior:**
   - Reads `stdin` to string up to EOF.
   - If `stdin` is empty, unparseable JSON, or lacks tool invocation details, fails open immediately emitting `{"decision":"allow"}` and exiting 0.
   - Extracts tool name and arguments map from varied agent hook payloads (`toolCall`, `tool_call`, top-level `toolName`/`tool_name`/`tool`, and `args`/`arguments`/`parameters`/`tool_input`).
   - Evaluates command execution via `superstate_core::command_guard::evaluate_command_guard`.
   - Emits decision JSON followed by newline to standard output and exits with code 0.

---

## TDD Execution

### 1. Red Phase
Created 13 integration tests in `crates/superstate-cli/tests/test_cli_command_guard.rs` validating end-to-end CLI behavior against `superstate guard pre-command`:
- `test_empty_stdin_allows`: Empty stdin fails open to `{"decision":"allow"}` with exit code 0.
- `test_invalid_json_allows`: Malformed JSON fails open to `{"decision":"allow"}` with exit code 0.
- `test_non_polling_tool_call_allows`: Non-polling tools (`view_file`, `manage_task` with action `kill`) return `{"decision":"allow"}`.
- `test_run_command_first_invocation_allows`: First run of `ps aux` returns `{"decision":"allow"}`.
- `test_run_command_second_invocation_within_cooldown_denies`: Immediate repeat of `ps aux` within 30s returns `{"decision":"deny"}` with cooldown message.
- `test_run_command_distinct_command_allows`: Distinct command execution (`cargo build` then `cargo test`) allows immediately.
- `test_run_command_exact_duplicate_whitespace_normalization`: Duplicate commands differing only in whitespace (`cargo   check   --all` vs `cargo check --all`) are normalized and denied.
- `test_debounce_sec_customization`: Custom `--debounce-sec 1` flag allows execution after the 1s cooldown window elapses.
- `test_manage_task_status_blocks_within_cooldown`: Repeat `manage_task` with `Action: "status"` is denied within cooldown.
- `test_manage_task_list_shares_cooldown_with_status`: `Action: "list"` shares the cooldown key with `status` and is denied.
- `test_manage_task_non_polling_action_allows`: Non-polling action `Action: "kill"` is allowed repeatedly.
- `test_procfs_polling_blocks_within_cooldown`: Procfs status polling (`/proc/4211/status` then `/proc/8842/cmdline`) is debounced.
- `test_flexible_payload_formats`: Claude Code hook style (`tool_name`/`tool_input`) and snake_case (`tool_call`) payloads are parsed and guarded accurately.

Ran `cargo test -p superstate --test test_cli_command_guard` to verify RED failure:
```text
failures:
    test_debounce_sec_customization
    test_empty_stdin_allows
    test_flexible_payload_formats
    test_invalid_json_allows
    test_manage_task_list_shares_cooldown_with_status
    test_manage_task_non_polling_action_allows
    test_manage_task_status_blocks_within_cooldown
    test_non_polling_tool_call_allows
    test_procfs_polling_blocks_within_cooldown
    test_run_command_distinct_command_allows
    test_run_command_exact_duplicate_whitespace_normalization
    test_run_command_first_invocation_allows
    test_run_command_second_invocation_within_cooldown_denies

test result: FAILED. 0 passed; 13 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.46s
```
All 13 tests failed with expected error: `error: unrecognized subcommand 'pre-command'`.

### 2. Green Phase
1. In `crates/superstate-cli/src/commands/guard.rs`:
   - Added `PreCommand(PreCommandArgs)` to `GuardCommands`.
   - Defined `PreCommandArgs` with `debounce_sec` (default: 30) and `state_path`.
   - Generalized `emit_decision<T: serde::Serialize>(decision: &T)` to handle both file and command guard decisions.
   - Added `tool` key fallback in `extract_from_container`.
   - Implemented `cmd_pre_command(args: PreCommandArgs)` and wired `GuardCommands::PreCommand` in `dispatch_guard`.
2. In `crates/superstate-cli/src/state_cli.rs`:
   - Added `Guard` subcommand variant to `StateCommands`.
   - Dispatched `StateCommands::Guard { cmd }` to `commands::guard::dispatch_guard(cmd)`.

Ran `cargo test -p superstate --test test_cli_command_guard`:
```text
running 13 tests
test test_empty_stdin_allows ... ok
test test_invalid_json_allows ... ok
test test_run_command_first_invocation_allows ... ok
test test_non_polling_tool_call_allows ... ok
test test_manage_task_non_polling_action_allows ... ok
test test_procfs_polling_blocks_within_cooldown ... ok
test test_run_command_distinct_command_allows ... ok
test test_manage_task_list_shares_cooldown_with_status ... ok
test test_flexible_payload_formats ... ok
test test_manage_task_status_blocks_within_cooldown ... ok
test test_run_command_exact_duplicate_whitespace_normalization ... ok
test test_run_command_second_invocation_within_cooldown_denies ... ok
test test_debounce_sec_customization ... ok

test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.61s
```
All 13 integration tests passed (GREEN).

### 3. Verification & Static Analysis
- Regression suite `cargo test -p superstate --test test_cli_guard`: All 12 tests passed.
- Entire crate test suite `cargo test -p superstate`: All unit and integration tests passed.
- Clippy analysis `cargo clippy -p superstate --all-targets -- -D warnings`: Passed with 0 warnings.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-cli/src/commands/guard.rs` | Modified | Added `PreCommand` variant, `PreCommandArgs`, `cmd_pre_command` execution logic, and generic `emit_decision` |
| `crates/superstate-cli/src/state_cli.rs` | Modified | Added `StateCommands::Guard` variant and routed in `dispatch` |
| `crates/superstate-cli/tests/test_cli_command_guard.rs` | Created | Added 13 end-to-end integration tests covering CLI invocation, debouncing, arguments, and fail-open behaviors |

---

## Commit Details

- **Commit SHA:** `4c1afbb7fa8fa7652758bfd3215aa6f5193f41eb` (`4c1afbb`)
- **Message:** `feat(cli): add superstate guard pre-command CLI handler`
- **GPG Verification:** Skipped (`--no-gpg-sign`) per repository instructions.
