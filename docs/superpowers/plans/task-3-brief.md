# Task 3: Interactive REPL Module with Live Event Streaming and Slash Commands

## Goal
Implement the interactive terminal REPL harness in `crates/superstate-cli/src/repl.rs`, expose it in `crates/superstate-cli/src/lib.rs`, and write unit and integration tests in `crates/superstate-cli/tests/test_repl_commands.rs`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## Files
- Create: `crates/superstate-cli/src/repl.rs`
- Modify: `crates/superstate-cli/src/lib.rs` (add `pub mod repl;`)
- Create: `crates/superstate-cli/tests/test_repl_commands.rs`

## Detailed Implementation
1. Create `crates/superstate-cli/src/repl.rs`:
   - `ReplConfig`:
     ```rust
     #[derive(Debug, Clone)]
     pub struct ReplConfig {
         pub repo_root: std::path::PathBuf,
         pub model: Option<String>,
         pub gate: Option<String>,
         pub context_strategy: String,
         pub container: Option<String>,
         pub max_turns: usize,
         pub max_gates: usize,
         pub rollback: bool,
     }
     ```
   - `ReplSession`:
     ```rust
     pub struct ReplSession {
         pub repo_root: std::path::PathBuf,
         pub gate: Option<String>,
         pub model: String,
         pub context_strategy: String,
         pub state: superstate_agent::state::State,
         pub history: Vec<superstate_provider::Message>,
     }
     ```
   - `ReplControl`:
     ```rust
     #[derive(Debug, PartialEq, Eq)]
     pub enum ReplControl {
         Handled,
         Exit,
         SubmitPrompt(String),
     }
     ```
   - `handle_repl_command`:
     Processes slash commands:
     - `/help`: prints table of slash commands (`/help`, `/state`, `/diff`, `/review`, `/doctor`, `/gate`, `/clear`, `/exit`).
     - `/state`: prints current SKILL.state: Orchestrator, Runtime, and Agent json fields.
     - `/diff`: executes `git diff` in `session.repo_root` and prints output.
     - `/review`: runs review engine or prints status of uncommitted diff.
     - `/doctor`: runs doctor verification handshake.
     - `/gate`: if argument provided (`/gate cargo test`), updates `session.gate`; otherwise prints current gate.
     - `/clear`: clears `session.history` and resets working state.
     - `/exit` or `/quit`: returns `ReplControl::Exit`.
     - Returns `ReplControl::Handled` for handled slash commands.
     - If line does not start with `/`, returns `ReplControl::SubmitPrompt(line.to_string())`.
   - `render_banner`:
     Prints ASCII banner, Superstate version, active repository root, model, gate, and detected toolchains.
   - `format_event`:
     Formats `superstate_agent::run::Event`:
     - `Turn`: displays step number and turn preview.
     - `Tool`: `● <name> <input> -> <summary>` (highlighted in red if `is_error`).
     - `Gate`: `✓ Gate passed` or `✗ Gate failed`.
     - `RolledBack`: `↺ Rolled back working tree to <to>`.
   - `run_repl_stream<R: std::io::BufRead, W: std::io::Write>(mut reader: R, mut writer: W, mut session: ReplSession) -> Result<(), String>`:
     Loops reading lines from `reader`:
     - Writes prompt `superstate > ` and flushes.
     - Parses line via `handle_repl_command`.
     - Breaks on `ReplControl::Exit` or EOF.
     - On `ReplControl::SubmitPrompt(prompt)`:
       Logs action, updates state/history, and if a provider is configured, executes turn or logs dispatch.
   - `pub fn run_repl(config: ReplConfig) -> Result<(), String>`:
     Initializes `ReplSession` with environment inspection (probes git branch, available tools), calls `render_banner`, and runs `run_repl_stream` on `std::io::stdin().lock()` and `std::io::stdout()`.

2. Expose in `crates/superstate-cli/src/lib.rs`:
   Add `pub mod repl;`.

3. Tests in `crates/superstate-cli/tests/test_repl_commands.rs`:
   - `test_slash_help`: verifies `/help` outputs command guide.
   - `test_slash_gate_get_and_set`: verifies `/gate` inspects and sets gate command.
   - `test_slash_state_inspection`: verifies `/state` outputs Orchestrator, Runtime, and Agent state.
   - `test_slash_exit`: verifies `/exit` and `/quit` return `ReplControl::Exit`.
   - `test_repl_stream_driving`: feeds commands `/help`, `/gate cargo test`, `/state`, `/exit` through a `Cursor` and asserts clean execution.

4. Verify:
   `cargo test -p superstate --test test_repl_commands`
   `cargo check --workspace --all-targets`

5. Commit:
   ```bash
   git add crates/superstate-cli/src/repl.rs crates/superstate-cli/src/lib.rs crates/superstate-cli/tests/test_repl_commands.rs
   git commit --no-gpg-sign -m "feat(cli): add interactive REPL module with live event streaming and slash commands"
   ```
6. Run `roborev status; roborev show HEAD`.
7. Write report to `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness/docs/superpowers/plans/task-3-report.md`.
