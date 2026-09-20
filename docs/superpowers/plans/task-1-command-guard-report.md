# Task 1 Report: Core Polling Guard & Cooldown State Engine in `crates/superstate-core`

**Status:** Completed (DONE)  
**Commit SHA:** `65d301087224073d7f2479b4205dcb671356bd2b` (`65d3010`)  
**Branch:** `feat/pre-tool-use-command-guard`  
**Repository:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-command-guard`  
**Date:** 2026-09-20  

---

## Executive Summary

Task 1 implemented the core deterministic `PreToolUse` command and polling guard engine in `crates/superstate-core/src/command_guard.rs` and re-exported its types and function in `crates/superstate-core/src/lib.rs`.

The guard intercepts tool invocations before execution and evaluates them against an active cooldown history map:
1. **Tool Classification:**
   - **`manage_task`**: Inspects `Action` or `action`. If `"status"` or `"list"` (case-insensitive), classifies as `"manage_task_status"`. Non-polling actions (e.g. `"kill"`, `"send_input"`) pass through as `Allow` immediately without touching state.
   - **Shell Execution Tools** (`run_command`, `bash`, `sh`, `execute_command`, `executecommand`, `shell`, `exec_command`, `run_shell_command`, `terminal`):
     - Procfs polling regex (`/proc/(\d+|\*)/(cmdline|stat|status)`) -> classified as `"procfs_polling"`.
     - Process inspection regex (`\b(ps\s+aux|ps\s+-ef|pgrep|top|htop|jobs)\b`) -> classified as `"process_inspection"`.
     - Exact duplicate detection -> normalized command representation `cmd:<normalized>`.
   - **Unrelated Tools** (e.g. `view_file`): Allowed unconditionally without touching state files.
2. **Cooldown & State Engine:**
   - Default debounce window is 30 seconds (`CommandGuardConfig::default().debounce_sec = 30`).
   - Default state path: `.agent-state/command_guard_history.json`.
   - If an entry exists and `now - last < debounce_sec`: Returns `CommandGuardDecision::Deny { reason }` instructing the LLM:
     `[superstate-guard] Busy-wait polling blocked: command was executed {elapsed}s ago. Cooldown active ({remaining}s remaining). Do not loop on status; yield turn to await background task completion notification.`
   - If allowed: Updates timestamp to `now`, prunes entries older than $2 \times \text{debounce\_sec}$ to bound state file footprint, and atomically persists JSON to disk.
3. **Fail-Open Policy:**
   - File I/O errors, unparseable/corrupted JSON, or unwritable paths return `CommandGuardDecision::Allow` and never panic.

---

## TDD Execution

### 1. Red Phase
Created 15 integration unit tests in `crates/superstate-core/tests/test_command_guard.rs` covering:
- `test_config_default`: Default debounce is 30s and state path is None.
- `test_serde_command_guard_decision`: Serde roundtrip for Allow (`{"decision":"allow"}`) and Deny (`{"decision":"deny","reason":"..."}`).
- `test_unrelated_tool_allows_immediately`: Non-command tools (e.g. `view_file`) allow immediately without touching disk.
- `test_manage_task_status_blocks_within_cooldown`: Immediate second status poll is denied with expected cooldown message.
- `test_manage_task_list_shares_cooldown_with_status`: `list` shares cooldown key with `status`.
- `test_manage_task_non_polling_action_allows`: `kill` and `send_input` allow without debounce.
- `test_run_command_procfs_polling_blocks`: Procfs patterns (`/proc/1234/status`, `/proc/1234/cmdline`, `/proc/*/stat`) trigger cooldown.
- `test_run_command_process_inspection_blocks`: `ps aux`, `ps -ef`, `pgrep` trigger cooldown; non-process commands with word boundaries (`stop_server.sh`) pass through.
- `test_run_command_exact_duplicate_blocks`: Duplicate commands are blocked; distinct commands are allowed.
- `test_run_command_exact_duplicate_whitespace_normalization`: Commands differing only in whitespace normalize to the same key and block.
- `test_cooldown_expiry_allows_subsequent_execution`: Execution allowed and timestamp updated after cooldown expires.
- `test_prunes_entries_older_than_twice_debounce`: Stale entries older than $2 \times \text{debounce\_sec}$ are purged on update.
- `test_fail_open_on_corrupted_state_file`: Corrupted JSON recovers gracefully and returns `Allow`.
- `test_fail_open_on_unwritable_directory`: Unwritable file path returns `Allow` without panicking.
- `test_case_insensitivity_of_tool_and_arg_keys`: Uppercase tool names and varied argument key casings work seamlessly.

Ran `cargo test -p superstate-core --test test_command_guard` to verify RED failure:
```text
error[E0432]: unresolved import `superstate_core::command_guard`
 --> crates/superstate-core/tests/test_command_guard.rs:4:22
  |
4 | use superstate_core::command_guard::{
  |                      ^^^^^^^^^^^^^ could not find `command_guard` in `superstate_core`
error: could not compile `superstate-core` (test "test_command_guard") due to 1 previous error
```

### 2. Green Phase
1. Added `regex = "1.11"` to `crates/superstate-core/Cargo.toml`.
2. Implemented `CommandGuardConfig`, `CommandGuardDecision`, and `evaluate_command_guard` in `crates/superstate-core/src/command_guard.rs`:
   - Precompiled procfs and process inspection regexes via `std::sync::LazyLock`.
   - Extracted tool arguments with case-insensitive key lookups.
   - Evaluated cooldown timestamps with saturating arithmetic.
   - Atomic state file persistence via temporary file rename.
3. Exported `command_guard` module and items in `crates/superstate-core/src/lib.rs`.

Ran `cargo test -p superstate-core --test test_command_guard`:
```text
running 15 tests
test test_config_default ... ok
test test_manage_task_non_polling_action_allows ... ok
test test_serde_command_guard_decision ... ok
test test_fail_open_on_unwritable_directory ... ok
test test_unrelated_tool_allows_immediately ... ok
test test_manage_task_status_blocks_within_cooldown ... ok
test test_manage_task_list_shares_cooldown_with_status ... ok
test test_run_command_exact_duplicate_whitespace_normalization ... ok
test test_fail_open_on_corrupted_state_file ... ok
test test_case_insensitivity_of_tool_and_arg_keys ... ok
test test_run_command_procfs_polling_blocks ... ok
test test_cooldown_expiry_allows_subsequent_execution ... ok
test test_run_command_exact_duplicate_blocks ... ok
test test_run_command_process_inspection_blocks ... ok
test test_prunes_entries_older_than_twice_debounce ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

### 3. Static Analysis & Full Workspace Verification
- `cargo clippy -p superstate-core --all-targets -- -D warnings`: Passed with 0 warnings.
- `cargo test -p superstate-core`: All unit and integration test suites in `superstate-core` passed.
- `cargo check --all`: Entire workspace compiled cleanly.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-core/Cargo.toml` | Modified | Added `regex = "1.11"` dependency |
| `crates/superstate-core/src/command_guard.rs` | Created | Implemented command guard evaluation, classification, and cooldown state engine |
| `crates/superstate-core/src/lib.rs` | Modified | Declared and re-exported `command_guard` types and function |
| `crates/superstate-core/tests/test_command_guard.rs` | Created | Added 15 unit tests covering classification, cooldown, pruning, and fail-open behaviors |

---

## Commit Details

- **Commit SHA:** `65d301087224073d7f2479b4205dcb671356bd2b` (`65d3010`)
- **Message:** `feat(core): implement PreToolUse command and polling guard engine`
- **GPG Verification:** Skipped (`--no-gpg-sign`) per repository instructions.
