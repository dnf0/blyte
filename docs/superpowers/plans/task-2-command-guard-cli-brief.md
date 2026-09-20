# Task 2 Brief: CLI Subcommand `superstate guard pre-command` in `superstate-cli`

## Workspace & Context
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-command-guard`
- **Feature Branch:** `feat/pre-tool-use-command-guard`
- **Spec:** `docs/superpowers/specs/2026-09-20-pre-tool-use-command-polling-guard-design.md`
- **Plan:** `docs/superpowers/plans/2026-09-20-pre-tool-use-command-polling-guard.md`

## Objective
Implement and wire the `pre-command` CLI subcommand under `superstate guard` in `crates/superstate-cli` that reads toolCall payloads from `stdin` and emits the PreToolUse decision JSON to `stdout`.

## Files to Create / Modify
- **Modify:** `crates/superstate-cli/src/commands/guard.rs` (add `PreCommand(PreCommandArgs)` and execution handler)
- **Modify:** `crates/superstate-cli/src/state_cli.rs` (ensure `GuardCommands::PreCommand` is routed)
- **Create Test:** `crates/superstate-cli/tests/test_cli_command_guard.rs`
- **Report:** `docs/superpowers/plans/task-2-command-guard-cli-report.md`

## Interface Specification
```rust
#[derive(Subcommand, Debug, Clone)]
pub enum GuardCommands {
    /// Evaluate file read operations against deterministic line boundaries
    PreToolUse(PreToolUseArgs),
    /// Evaluate command and task tool invocations against cooldown debounce
    PreCommand(PreCommandArgs),
}

#[derive(Args, Debug, Clone)]
pub struct PreCommandArgs {
    /// Cooldown window in seconds before allowing repeated polling queries
    #[arg(long, default_value_t = 30)]
    pub debounce_sec: u64,

    /// Optional path to state file tracking execution history
    #[arg(long)]
    pub state_path: Option<PathBuf>,
}
```

## CLI Execution Logic
1. Reads `stdin` into a buffer (up to EOF).
2. Parses root JSON container via `serde_json::from_str`.
   - If invalid JSON or empty, emit `{"decision":"allow"}` and return `Ok(())` (fail-open).
3. Uses existing `extract_tool_invocation(&root)` to extract `(tool_name, args)`.
   - If no tool invocation can be parsed, emit `{"decision":"allow"}` and return `Ok(())`.
4. Constructs `CommandGuardConfig { debounce_sec: args.debounce_sec, state_path: args.state_path }`.
5. Calls `superstate_core::command_guard::evaluate_command_guard(&tool_name, &args, &config)`.
6. Emits `decision` JSON to `stdout` (`{"decision":"allow"}` or `{"decision":"deny","reason":"..."}`).

## Required TDD Steps
1. Write failing integration tests in `crates/superstate-cli/tests/test_cli_command_guard.rs`:
   - Test piping valid toolCall JSON for `run_command` via stdin.
   - Test second invocation within 30s returns `{"decision":"deny",...}` on stdout.
   - Test piping toolCall for distinct command returns `{"decision":"allow"}`.
   - Test `--debounce-sec` flag customization.
   - Test empty or invalid stdin fails open to `{"decision":"allow"}`.
2. Run `cargo test -p superstate-cli --test test_cli_command_guard` (verify RED failure).
3. Implement `pre-command` handler in `crates/superstate-cli/src/commands/guard.rs` and `state_cli.rs`.
4. Run `cargo test -p superstate-cli --test test_cli_command_guard` (verify GREEN pass).
5. Run `cargo clippy -p superstate-cli --all-targets -- -D warnings`.
6. Commit changes: `git commit -m "feat(cli): add superstate guard pre-command CLI handler" --no-gpg-sign`.
7. Write report to `docs/superpowers/plans/task-2-command-guard-cli-report.md`.
