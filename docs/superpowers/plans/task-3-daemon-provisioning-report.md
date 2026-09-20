# Task 3: Daemon Automated Provisioning & Untracked Hygiene Report

## Overview
This report documents the design, implementation, and verification of **Task 3: Daemon Automated Provisioning & Untracked Hygiene** in `superstate`.

Task 3 integrates the command guard functionality developed in Tasks 1 and 2 directly into the autonomous daemon lifecycle (`DaemonRunner::step`), orchestrating PreToolUse hook configuration and git exclude hygiene seamlessly.

## Key Changes Implemented

### 1. `DaemonConfig` Updates (`crates/superstate-core/src/daemon.rs`)
- Added `pub guard_commands: bool` (default: `true`, serialized with `#[serde(default = "default_true")]`).
- Updated `impl Default for DaemonConfig` to include `guard_commands: true`.

### 2. Autonomous Hook Provisioning & Merging (`crates/superstate-core/src/daemon.rs`)
- Updated `DaemonRunner::step()`:
  - Evaluates `if self.config.guard_file_reads || self.config.guard_commands`.
  - Ensures the workspace `.agents/` directory exists.
  - Inspects `.agents/hooks.json` if it already exists: parses existing JSON as `serde_json::Value::Object(map)` preserving preexisting hooks.
  - Merges active guards idempotently:
    - `"superstate-file-guard"`: matcher `view_file|View|read_file` invoking `superstate guard pre-tool-use --threshold-lines 200`.
    - `"superstate-command-guard"`: matcher `run_command|manage_task` invoking `superstate guard pre-command --debounce-sec 30`.
  - Ensures `.agents/` is appended to `.git/info/exclude` if `.git/info` exists and it is not already excluded, preventing inadvertent git tracking.

### 3. CLI Flags & Argument Forwarding (`crates/superstate-cli/src/`)
- Updated `DaemonArgs` in `crates/superstate-cli/src/state_cli.rs`:
  - Added `--no-file-guard` flag (`pub no_file_guard: bool`).
  - Added `--no-command-guard` flag (`pub no_command_guard: bool`).
  - Updated `--no-guard` doc/help string to clarify that it disables all automated PreToolUse guards.
  - Implemented helper methods:
    - `pub fn use_guard(&self) -> bool { !self.no_guard && !self.no_file_guard }`
    - `pub fn use_command_guard(&self) -> bool { !self.no_guard && !self.no_command_guard }`
- Updated `cmd_daemon` in `crates/superstate-cli/src/commands/daemon.rs`:
  - Accepts `guard_commands: bool`.
  - Initializes `DaemonConfig.guard_commands`.
  - Detached daemon process spawning correctly forwards `--no-guard` (when both disabled), `--no-file-guard`, and `--no-command-guard`.
- Updated call sites in `crates/superstate-cli/src/main.rs`, `state_cli.rs`, `handoff.rs`, and integration tests.

### 4. Integration Test Coverage
- `crates/superstate-core/tests/test_daemon_guard.rs`:
  - `test_daemon_config_default_guard_commands`: verifies `guard_commands` defaults to `true`.
  - `test_daemon_config_deserialize_defaults_guard_commands_to_true`: verifies JSON deserialization defaults `guard_commands` to `true`.
  - `test_daemon_runner_provisions_hooks_json`: verifies provisioning of both file guard and command guard entries in `.agents/hooks.json`.
  - `test_daemon_provisions_command_guard_in_hooks_json`: verifies specific command guard structure and args.
  - `test_daemon_no_command_guard_disables_provisioning`: verifies only file guard is written when `guard_commands: false`.
  - `test_daemon_runner_no_guard_does_not_provision_hooks_json`: verifies no hooks file created when both disabled.
  - `test_daemon_no_guard_disables_both`: verifies `--no-guard` suppresses both hooks.
  - `test_daemon_merges_hooks_when_file_exists`: verifies existing custom hooks are preserved while missing guards are inserted.
- `crates/superstate-cli/tests/test_cli_daemon_guard.rs`:
  - `test_cli_no_guard_flag_parsing`
  - `test_cli_no_guard_flag_default_false`
  - `test_cli_no_command_guard_flag_parsing`
  - `test_cli_no_command_guard_default_false`
  - `test_cli_no_file_guard_flag_parsing`
  - `test_cli_no_guard_disables_both`

## Verification Evidence
1. **Targeted Integration Tests:**
   - `cargo test -p superstate-core --test test_daemon_guard`: **12 passed, 0 failed**.
   - `cargo test -p superstate --test test_cli_daemon_guard`: **6 passed, 0 failed**.
2. **Full Workspace Test Suite:**
   - `cargo test --all`: **All tests passed** across all workspace crates (`superstate-core`, `superstate-cli`, `superstate-worktree`, `superstate-mcp`, `superstate-review`, `superstate-rules`, `superstate-syntax`).
3. **Clippy Static Analysis:**
   - `cargo clippy --all-targets -- -D warnings`: **Clean, zero warnings**.
4. **Binary Installation & CLI Verification:**
   - `cargo install --path crates/superstate-cli --locked --force`: Successfully built release binary and replaced `/Users/danielfisher/.cargo/bin/superstate`.
   - `superstate daemon --help`: Verified `--no-guard`, `--no-file-guard`, and `--no-command-guard` options are rendered in help output.

## Git Commit
- **Commit Hash:** `ba0818c`
- **Commit Message:** `feat(daemon): provision automated command polling guard in workspace hooks`
