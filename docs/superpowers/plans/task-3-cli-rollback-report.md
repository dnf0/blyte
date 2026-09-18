# Task 3 Report: CLI Flag `--auto-rollback` and Detachment Forwarding

**Status:** Completed  
**Commit SHA:** `1695135` (`16951353c983a54b35fd62c764e5c54e0b5d5d85`)  
**Branch:** `feat/rapid-canary-eval-and-rollback`  
**Repository:** `/Users/danielfisher/repos/superstate`  
**Date:** 2026-09-18  

---

## Executive Summary

Task 3 surfaced the deterministic git rollback functionality implemented in Task 2 to the command-line interface in `crates/superstate-cli`, as specified in `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-3-cli-rollback-brief.md`.

Autonomous daemon workers executing under `superstate daemon` and `state daemon` can now receive `--auto-rollback` to automatically roll back corrupted working trees to the milestone's baseline commit and remove untracked debris upon verification failure. Furthermore:
1. When detached background daemons are spawned with `--detach`, the `--auto-rollback` flag is explicitly forwarded to the child daemon process.
2. An environment variable fallback (`SUPERSTATE_AUTO_ROLLBACK=1` or `true`) enables rollback even when workers or wrappers omit the explicit CLI flag.
3. Default behavior remains safe and non-breaking (`auto_rollback: false`).

---

## TDD Execution

### 1. Red Phase
Created comprehensive integration test suite in `crates/superstate-cli/tests/test_cli_rollback.rs` testing 7 distinct behaviors:
1. `test_cli_state_daemon_parses_auto_rollback_flag`: Verifies clap parses `--auto-rollback` into `DaemonArgs.auto_rollback = true`.
2. `test_cli_state_daemon_auto_rollback_default_false`: Verifies default value is `false`.
3. `test_cli_daemon_help_has_auto_rollback`: Verifies both `superstate daemon --help` and `superstate state daemon --help` document `--auto-rollback`.
4. `test_cli_daemon_auto_rollback_execution`: End-to-end test executing a corrupted driver with `--auto-rollback`, verifying tracked files are restored to initial commit, untracked debris is removed by `git clean -fd`, and `[rollback]` appears in the epistemic ledger.
5. `test_cli_daemon_auto_rollback_disabled_by_default`: Verifies corrupted state is left in place when `--auto-rollback` is omitted.
6. `test_cli_daemon_auto_rollback_via_env_var`: Verifies environment variable `SUPERSTATE_AUTO_ROLLBACK=1` activates rollback without CLI flag.
7. `test_cli_daemon_detached_forwarding`: Verifies that `superstate state daemon --detach --auto-rollback` passes `--auto-rollback` to the background child process, which executes rollback cleanly.

Ran `cargo test -p superstate --test test_cli_rollback` and confirmed expected RED failure:
```text
error[E0609]: no field `auto_rollback` on type `DaemonArgs`
  --> crates/superstate-cli/tests/test_cli_rollback.rs:57:22
   |
57 |                 args.auto_rollback,
   |                      ^^^^^^^^^^^^^ unknown field
```

### 2. Green Phase
Implemented production changes across `crates/superstate-cli`:
- **CLI Argument Definition** (`crates/superstate-cli/src/state_cli.rs`):
  Added `auto_rollback` to `DaemonArgs`:
  ```rust
  /// Automatically roll back working tree to milestone baseline on verification failure
  #[arg(
      long,
      help = "Automatically roll back working tree to milestone baseline on verification failure"
  )]
  pub auto_rollback: bool,
  ```
- **CLI Dispatch and Top-Level Binary** (`crates/superstate-cli/src/state_cli.rs`, `crates/superstate-cli/src/main.rs`):
  Forwarded `args.auto_rollback` into `commands::daemon::cmd_daemon`.
- **Command Handler and Detachment Forwarding** (`crates/superstate-cli/src/commands/daemon.rs`):
  - Updated `cmd_daemon` signature to take `auto_rollback: bool` with `#[allow(clippy::too_many_arguments)]`.
  - Added environment variable fallback:
    ```rust
    let auto_rollback = auto_rollback
        || std::env::var("SUPERSTATE_AUTO_ROLLBACK")
            .map(|v| {
                let s = v.trim();
                s == "1" || s.eq_ignore_ascii_case("true")
            })
            .unwrap_or(false);
    ```
  - Forwarded `--auto-rollback` when spawning background child process:
    ```rust
    if auto_rollback {
        cmd.arg("--auto-rollback");
    }
    ```
  - Configured `auto_rollback` in `DaemonConfig`:
    ```rust
    let config = DaemonConfig {
        ...
        auto_rollback,
    };
    ```
- **Callers Updated**:
  - `crates/superstate-cli/src/commands/handoff.rs`: passed `false` as default for `auto_rollback`.
  - `crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs`: passed `false` at existing test invocation points.

### 3. Verification & Linter Checks
1. **Targeted Integration Test**:
   `cargo test -p superstate --test test_cli_rollback` -> **7 passed; 0 failed**.
2. **Full Workspace Test Suite**:
   `cargo test --all` -> **All packages and tests passed (100% clean)**.
3. **Strict Clippy Lint**:
   `cargo clippy --all-targets -- -D warnings` -> **0 errors, 0 warnings (100% clean)**.
4. **Binary Reinstallation**:
   `cargo install --path crates/superstate-cli --locked --force` -> Installed and replaced `/Users/danielfisher/.cargo/bin/state` and `/Users/danielfisher/.cargo/bin/superstate`.
   Verified `superstate daemon --help` and `state daemon --help` display `--auto-rollback`.

---

## Git Commit Details

- **Commit**: `1695135` (`16951353c983a54b35fd62c764e5c54e0b5d5d85`)
- **Message**: `feat(cli): wire --auto-rollback flag and detachment forwarding`
- **Files Modified**:
  - `crates/superstate-cli/src/state_cli.rs`
  - `crates/superstate-cli/src/commands/daemon.rs`
  - `crates/superstate-cli/src/commands/handoff.rs`
  - `crates/superstate-cli/src/main.rs`
  - `crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs`
  - `crates/superstate-cli/tests/test_cli_rollback.rs` (new test)
