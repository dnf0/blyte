# Task 2 Report: CLI Argument Parser with Dual-Mode Support

## Status
DONE

## Commit
- SHA: `c8a9ee59ea7871d9d0b4c9495aa33d18edb3c62b`
- Message: `feat(cli): add dual-mode prompt and execution flags to CLI parser`

## Actions Taken
1. Created `crates/superstate-cli/src/cli.rs`:
   - Defined `Cli` with optional positional `prompt`, execution flags (`--gate`, `--context`, `--model`, `--container`, `--max-turns`, `--max-gates`, `--rollback`), and optional subcommand `command: Option<Commands>`.
   - Added `args_conflicts_with_subcommands = true` to prevent ambiguous parsing between single-shot prompt and subcommands.
   - Configured `context` with `value_parser = ["state-only", "conversational"]` for strict validation.
   - Configured `rollback` with `require_equals = true`, `num_args = 0..=1`, and `BoolishValueParser` to cleanly support `--rollback`, `--rollback=true`, and `--rollback=false` regardless of argument ordering.
   - Derived `clap::Parser, Debug, PartialEq` on `Cli` and `clap::Subcommand, Debug, PartialEq` on `Commands`.
2. Derived `PartialEq` across underlying CLI argument structs:
   - Added `PartialEq` derive to `StateCommands`, `InitArgs`, `ListArgs`, `ShowArgs`, `DaemonSubcommands`, `DaemonArgs`, `CancelArgs`, `LogsArgs`, `HandoffArgs` in `crates/superstate-cli/src/state_cli.rs`.
   - Added `PartialEq` derive to `WorktreeCommands` in `crates/superstate-cli/src/commands/worktree.rs`.
   - Added `PartialEq` derive to `InspectArgs` in `crates/superstate-cli/src/commands/inspect.rs`.
   - Added `PartialEq` derive to `GuardCommands`, `PreToolUseArgs`, `PreCommandArgs` in `crates/superstate-cli/src/commands/guard.rs`.
3. Exposed `cli` module and updated `crates/superstate-cli/src/main.rs`:
   - Added `pub mod cli;` in `crates/superstate-cli/src/lib.rs`.
   - Updated `main.rs` to import `Cli` and `Commands` from `superstate_cli::cli`, matching on `Some(Commands::...)` and stubbing headless task prompt execution vs. interactive REPL launch when `cli.command` is `None`.
4. Comprehensive integration tests in `crates/superstate-cli/tests/test_cli_dispatch.rs`:
   - `test_parse_no_args_launches_interactive`: Verifies default interactive REPL dispatch with default configuration (`conversational` context, `max_turns: 25`, `max_gates: 5`, `rollback: true`).
   - `test_parse_single_shot_prompt`: Verifies single-shot prompt with `--gate` and `--context`.
   - `test_parse_custom_execution_flags`: Verifies custom execution flags (`--model`, `--container`, `--max-turns`, `--max-gates`, `--rollback`).
   - `test_parse_rollback_disabled`: Verifies disabling and enabling rollback via `--rollback=false`, `--rollback=true`, and `--rollback`, both trailing and preceding positional prompts.
   - `test_context_validation`: Verifies strict rejection of invalid context strategies.
   - `test_parse_subcommand_doctor`: Verifies routing to `Commands::Doctor`.
   - `test_parse_subcommand_mcp`: Verifies routing to `Commands::Mcp`.
   - `test_parse_subcommand_state_init`: Verifies routing to `Commands::State(StateCommands::Init(...))` with full equality comparison.
   - `test_prompt_conflicts_with_subcommand`: Verifies positional prompts conflicting with subcommands are rejected by clap parser.
5. Verification:
   - `cargo test -p superstate --test test_cli_dispatch`: 9 passed, 0 failed.
   - `cargo check -p superstate`: Completed cleanly with 0 errors and 0 warnings.
   - `cargo check --workspace --all-targets`: Completed cleanly with 0 errors and 0 warnings.
6. Roborev Code Review:
   - Initial review identified boolean flag ergonomics and `PartialEq` reflexivity; fully addressed and refined.
   - Final review on commit `c8a9ee5`: PASSED with "No issues found." (exit code 0).
