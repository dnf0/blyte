# Task 4 Implementation Report: Headless Single-Shot Solver Integration

## Overview
Implemented the headless single-shot solver runner in `crates/superstate-cli/src/task_runner.rs`, exposed it via `crates/superstate-cli/src/lib.rs`, wired prompt dispatch and REPL fallback into `crates/superstate-cli/src/main.rs`, and connected prompt submission in the REPL event loop (`crates/superstate-cli/src/repl.rs`). Followed up on automated roborev code review findings to preserve session state and redirect stream output to the caller's writer.

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
   - Implemented `run_task_with_state_and_writer`:
     - Allows running the solver while preserving an existing mutable `State` (epistemic ledger, patches, attempts) across multiple turns.
     - Directs formatted live events into the provided generic `writer: &mut W` via a synchronized Mutex wrapper.

2. **`crates/superstate-cli/src/lib.rs`**:
   - Exposed `pub mod task_runner;`.

3. **`crates/superstate-cli/src/main.rs`**:
   - In `cli.command == None`:
     - Dispatches headless task execution via `task_runner::run_task` when a non-empty `prompt` is provided, exiting with 0 on `Outcome::Verified` or 1 on gate failure/error.
     - Fails fast with `Error: prompt cannot be empty` and exit code 1 if a prompt was provided with only whitespace.
     - Launches `repl::run_repl` when no prompt or subcommand is given.

4. **`crates/superstate-cli/src/repl.rs`**:
   - Augmented `ReplSession` with execution parameters (`container`, `max_turns`, `max_gates`, `rollback`) initialized from `ReplConfig`.
   - Updated default model fallback from `"claude-3-5-sonnet"` to `"gemini-2.5-flash"` to match `superstate-provider`.
   - Wired `ReplControl::SubmitPrompt(prompt)` in `run_repl_stream` to dispatch `task_runner::run_task_with_state_and_writer` with active session settings, updating `session.state.orchestrator.goal` and `session.state.orchestrator.verification_cmd`, and streaming events directly to `writer`.

5. **`crates/superstate-cli/tests/test_task_runner.rs`**:
   - Added thread synchronization (`ENV_LOCK`) for tests mutating process environment variables.
   - Added `test_task_config_from_cli`: asserts full CLI flag mapping to `TaskConfig`.
   - Added `test_task_config_defaults`: asserts default values.
   - Added `test_run_task_missing_api_key_returns_clear_error`: verifies explicit error message mentioning `GEMINI_API_KEY`.
   - Added `test_run_task_with_state_and_writer_preserves_state`: verifies state retention and writer sink.
   - Added `test_repl_stream_prompt_dispatch`: verifies REPL stream handles prompt submission and dispatches to task runner.

## Verification
- `cargo test -p superstate --test test_task_runner`: 5 passed, 0 failed.
- `cargo test -p superstate`: all CLI tests passed.
- `cargo test --workspace`: all workspace unit and integration tests passed.
- `cargo check --workspace --all-targets`: 0 errors, 0 warnings.
- `cargo clippy -p superstate --all-targets -- -D warnings`: 0 warnings.
- Commits:
  - `a06408c feat(cli): integrate headless single-shot solver and wire REPL prompt dispatch`
  - `e097519 fix(cli): preserve REPL state across prompts and redirect live event writer`
