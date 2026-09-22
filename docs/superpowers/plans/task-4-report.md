# Task 4 Implementation Report: Headless Single-Shot Solver Integration

## Overview
Implemented the headless single-shot solver runner in `crates/superstate-cli/src/task_runner.rs`, exposed it via `crates/superstate-cli/src/lib.rs`, wired prompt dispatch and REPL fallback into `crates/superstate-cli/src/main.rs`, and connected prompt submission in the REPL event loop (`crates/superstate-cli/src/repl.rs`).

## Changes Made
1. **`crates/superstate-cli/src/task_runner.rs`**:
   - Implemented `TaskConfig` struct capturing execution parameters: `prompt`, `gate`, `context_strategy`, `model`, `container`, `max_turns`, `max_gates`, `rollback`, `root`.
   - Implemented `From<&crate::cli::Cli> for TaskConfig` mapping CLI arguments and fallback values.
   - Implemented `run_task(config: TaskConfig) -> Result<(superstate_agent::run::Outcome, superstate_agent::run::RunReport), String>`:
     - Configures AI provider using `Gemini::from_env()` with custom model override support, returning clear actionable guidance (`Provider error: no API key: set GEMINI_API_KEY (or GOOGLE_API_KEY)`) if missing.
     - Selects context strategy: `superstate_agent::context::StateOnly` when `context_strategy == "state-only"`, otherwise `superstate_agent::context::Conversational`.
     - Selects shell & filesystem: `ContainerShell` & `ContainerFs` targeted at `/workspace` if container ID is configured, otherwise `LocalShell` & `LocalFs` in host working directory.
     - Initializes `superstate_agent::state::State`.
     - Connects `crate::repl::format_event` live event streaming.
     - Executes `superstate_agent::run::Runner::run(&mut state)`.

2. **`crates/superstate-cli/src/lib.rs`**:
   - Exposed `pub mod task_runner;`.

3. **`crates/superstate-cli/src/main.rs`**:
   - In `cli.command == None`:
     - Dispatches headless task execution via `task_runner::run_task` when a non-empty `prompt` is provided, exiting with 0 on `Outcome::Verified` or 1 on gate failure/error.
     - Launches `repl::run_repl` when no prompt or subcommand is given.

4. **`crates/superstate-cli/src/repl.rs`**:
   - Augmented `ReplSession` with execution parameters (`container`, `max_turns`, `max_gates`, `rollback`) initialized from `ReplConfig`.
   - Wired `ReplControl::SubmitPrompt(prompt)` in `run_repl_stream` to dispatch `task_runner::run_task` with active session settings and display turn/usage outcomes or execution errors.

5. **`crates/superstate-cli/tests/test_task_runner.rs`**:
   - Added `test_task_config_from_cli`: asserts full CLI flag mapping to `TaskConfig`.
   - Added `test_task_config_defaults`: asserts default values.
   - Added `test_run_task_missing_api_key_returns_clear_error`: verifies explicit error message mentioning `GEMINI_API_KEY`.
   - Added `test_repl_stream_prompt_dispatch`: verifies REPL stream handles prompt submission and dispatches to task runner.

## Verification
- `cargo test -p superstate --test test_task_runner`: 4 passed, 0 failed.
- `cargo test -p superstate --test test_repl_commands`: 7 passed, 0 failed.
- `cargo test --workspace`: all workspace unit and integration tests passed.
- `cargo check --workspace --all-targets`: 0 errors, 0 warnings.
- `cargo clippy -p superstate --all-targets -- -D warnings`: 0 warnings.
- `git commit --no-gpg-sign -m "feat(cli): integrate headless single-shot solver and wire REPL prompt dispatch"` (commit `a06408c`).
