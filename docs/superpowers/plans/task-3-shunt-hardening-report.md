# Task 3 Report: Shunt CLI Stdin Streaming & Exit Code Verification

**Status:** Completed  
**Commit SHA:** `b6c519a` (`b6c519a9307d6364a5c0d5d7e7d58a5e8fe3857c`)  
**Branch:** `feat/shunt-hardening`  
**Repository:** `/Users/danielfisher/repos/superstate`  
**Date:** 2026-09-20  

---

## Executive Summary

Task 3 hardened and verified the `superstate shunt -- <command> [args...]` CLI subcommand in `crates/superstate-cli`, as specified in `docs/superpowers/plans/task-3-shunt-hardening-brief.md`.

The implementation verified and hardened four key operational guarantees:
1. **Unblocked Stdin Streaming**: Standard input passed via pipes is streamed directly into the spawned child process without intermediate truncation, buffering bottlenecks, or userspace reader deadlocks (validated up to 128 KB, exceeding standard 64 KB OS pipe limits, as well as bidirectional streaming).
2. **Exact Exit Status Propagation**: The exact integer exit status code of the child process (including 0, 1, 42, 127, and POSIX signal terminations such as 137 for SIGKILL) is returned faithfully to the parent caller.
3. **Threshold-Based Output Distillation**: Outputs under the configured threshold (default 50 lines / 4 KB) pass through unaltered verbatim with zero formatting overhead; outputs exceeding threshold are distilled into `.agent-state/logs/` with high-signal summaries emitted to stdout.
4. **CLI Flag Configurability**: Added full support for `--logs-dir <DIR>` alongside `--threshold-lines <N>` on the `shunt` subcommand.

---

## TDD Execution

### 1. Red Phase
Authored comprehensive integration tests in `crates/superstate-cli/tests/test_cli_shunt.rs` covering:
- `test_cli_shunt_small_output_exit_code_zero`: Verifies small outputs pass through verbatim with exit code 0.
- `test_shunt_under_threshold_outputs_verbatim`: Verifies `superstate shunt -- sh -c 'echo "hello verbatim"'` emits verbatim stdout without distillation headers.
- `test_cli_shunt_propagates_non_zero_exit_code` / `test_shunt_preserves_exit_code`: Verifies exit code 42 is preserved.
- `test_shunt_preserves_exit_code_127`: Verifies standard command-not-found / custom exit code 127 is preserved.
- `test_cli_shunt_signal_exit_code`: Verifies POSIX signal terminations (SIGKILL = 128 + 9 = 137) propagate.
- `test_cli_shunt_large_output_truncates_and_saves_log` / `test_shunt_over_threshold_distills_success`: Verifies lines exceeding threshold produce `[superstate-shunt] Command succeeded (exit 0)` and disk log.
- `test_shunt_failure_distills_with_excerpt`: Verifies failing command over threshold emits failure excerpt, exit code 1, and log path.
- `test_shunt_forwards_stdin`: Verifies piped stdin (`test stdin content\n`) is forwarded to child `cat` and exit code 17 is preserved.
- `test_shunt_large_stdin_streaming`: Streams 128 KB (2 x 64 KB chunks) into `sh -c 'wc -c'`, confirming exactly 131,072 bytes pass without hanging or truncation.
- `test_shunt_bidirectional_streaming`: Verifies interactive stream transformation (`tr a-z A-Z`) concurrently processing stdin and stdout.
- `test_shunt_nonexistent_command_fails_cleanly`: Verifies unspawnable binaries exit 1 and report clear stderr errors.
- `test_shunt_custom_logs_dir`: Verifies `--logs-dir <path>` saves logs in the specified custom directory.

Ran `cargo test -p superstate --test test_cli_shunt`:
```text
failures:
    test_shunt_custom_logs_dir

test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.44s
```
Failure confirmed: `--logs-dir` was unrecognized by clap.

---

### 2. Green Phase
Implemented production updates in `crates/superstate-cli`:

1. **CLI Parser Schema** (`crates/superstate-cli/src/main.rs`):
   Added `logs_dir: Option<PathBuf>` to `Commands::Shunt`:
   ```rust
   Shunt {
       /// Line count threshold before shunting triggers
       #[arg(long, default_value = "50")]
       threshold_lines: usize,
       /// Directory where full unstripped command logs are stored
       #[arg(long)]
       logs_dir: Option<PathBuf>,
       /// Command and arguments to execute
       #[arg(trailing_var_arg = true, required = true)]
       cmd: Vec<String>,
   }
   ```
   Forwarded `logs_dir` from CLI dispatch to `commands::shunt::execute_shunt`.

2. **Command Implementation** (`crates/superstate-cli/src/commands/shunt.rs`):
   Updated `execute_shunt` signature to accept `logs_dir: Option<PathBuf>`.
   Configured `ShuntConfig`:
   ```rust
   let config = ShuntConfig {
       threshold_lines,
       threshold_bytes: 4096,
       logs_dir: logs_dir.unwrap_or_else(|| PathBuf::from(".agent-state/logs")),
   };
   ```
   Confirmed `Stdio::inherit()` cleanly forwards stdin file descriptors from the parent to child processes without userspace buffering or deadlock risk.

---

### 3. Verification & Quality Gates

1. **Targeted CLI Shunt Tests**:
   `cargo test -p superstate --test test_cli_shunt` -> **14 passed; 0 failed** (100% green).
2. **Full Workspace Test Suite**:
   `cargo test --all` -> **All workspace unit tests, integration tests, and doc tests passed**.
3. **Clippy Static Analysis**:
   `cargo clippy --all-targets -- -D warnings` -> **Clean (0 errors, 0 warnings)**.
4. **Host Binary Installation**:
   Ran `cargo install --path crates/superstate-cli --locked --force`, successfully installing `superstate` and `state` binaries into `/Users/danielfisher/.cargo/bin/`.
5. **CLI Verification**:
   Verified `superstate shunt --help` outputs the updated usage text including `--threshold-lines` and `--logs-dir`. Tested piped stdin execution with `echo "hello from installed shunt" | superstate shunt -- cat`.

---

## Deliverables

- `crates/superstate-cli/src/commands/shunt.rs`: Added `logs_dir` parameter support and configuration fallback.
- `crates/superstate-cli/src/main.rs`: Added `--logs-dir` argument to `Commands::Shunt`.
- `crates/superstate-cli/tests/test_cli_shunt.rs`: Expanded integration test suite to 14 tests covering exit codes, stdin streaming, large payloads, thresholds, distillation, and custom log directories.
- Host binaries installed: `/Users/danielfisher/.cargo/bin/superstate` and `/Users/danielfisher/.cargo/bin/state`.
- Conventional commit: `b6c519a` (`feat(cli): verify and harden superstate shunt stdin streaming and exit propagation`).
