# Task 4: Headless Single-Shot Solver Integration in crates/superstate-cli

## Goal
Implement headless single-shot solver integration in `crates/superstate-cli/src/task_runner.rs`, wire execution into `crates/superstate-cli/src/main.rs` (so `superstate "<prompt>"` runs headless, and `superstate` launches REPL), expose `task_runner` in `crates/superstate-cli/src/lib.rs`, and write unit / integration tests in `crates/superstate-cli/tests/test_task_runner.rs`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## Files
- Create: `crates/superstate-cli/src/task_runner.rs`
- Modify: `crates/superstate-cli/src/lib.rs` (add `pub mod task_runner;`)
- Modify: `crates/superstate-cli/src/main.rs` (wire `task_runner::run_task` when `prompt` is provided, and `repl::run_repl` when no prompt/subcommand)
- Modify: `crates/superstate-cli/src/repl.rs` (wire prompt submission in REPL loop to execute via `task_runner` when provider is available)
- Create: `crates/superstate-cli/tests/test_task_runner.rs`

## Detailed Implementation Steps
1. Create `crates/superstate-cli/src/task_runner.rs`:
   - `TaskConfig`:
     ```rust
     #[derive(Debug, Clone)]
     pub struct TaskConfig {
         pub prompt: String,
         pub gate: Option<String>,
         pub context_strategy: String,
         pub model: Option<String>,
         pub container: Option<String>,
         pub max_turns: usize,
         pub max_gates: usize,
         pub rollback: bool,
         pub root: std::path::PathBuf,
     }
     ```
   - Implement `From<&crate::cli::Cli> for TaskConfig`.
   - `run_task(config: TaskConfig) -> Result<(superstate_agent::run::Outcome, superstate_agent::run::RunReport), String>`:
     - Check `Gemini::from_env()`. If missing API key, return a clear error: `Provider error: no API key: set GEMINI_API_KEY (or GOOGLE_API_KEY)`.
     - Select context strategy: `"state-only"` => `superstate_agent::context::StateOnly`, else `superstate_agent::context::Conversational`.
     - Select shell: if `let Some(cid) = config.container`, `superstate_agent::shell::ContainerShell { container_id: cid, workdir: std::path::PathBuf::from("/workspace") }`, else `superstate_agent::shell::LocalShell`.
     - Initialize `state = superstate_agent::state::State::new(&config.prompt, config.gate.as_deref().unwrap_or(""))`.
     - Event logger: uses `crate::repl::format_event(&event)` to print progress as it happens.
     - Build `Runner` and call `runner.run(&mut state)`.
     - Return `(outcome, report)`.
2. Update `crates/superstate-cli/src/main.rs`:
   - When `cli.command` is `None`:
     - If `let Some(ref prompt) = cli.prompt`:
       - If prompt is not empty:
         ```rust
         let config = superstate_cli::task_runner::TaskConfig::from(&cli);
         match superstate_cli::task_runner::run_task(config) {
             Ok((superstate_agent::run::Outcome::Verified, report)) => {
                 println!("Task verified in {} turns (usage: {:?})", report.outcome_turns, report.usage);
                 std::process::exit(0);
             }
             Ok((outcome, report)) => {
                 eprintln!("Task finished with outcome: {outcome:?} in {} turns", report.outcome_turns);
                 std::process::exit(1);
             }
             Err(err) => {
                 eprintln!("Error executing task: {err}");
                 std::process::exit(1);
             }
         }
         ```
     - Else:
       - Run interactive REPL:
         ```rust
         let repl_config = superstate_cli::repl::ReplConfig {
             repo_root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
             model: cli.model,
             gate: cli.gate,
             context_strategy: cli.context,
             container: cli.container,
             max_turns: cli.max_turns,
             max_gates: cli.max_gates,
             rollback: cli.rollback,
         };
         if let Err(e) = superstate_cli::repl::run_repl(repl_config) {
             eprintln!("REPL error: {e}");
             std::process::exit(1);
         }
         ```
3. Update `crates/superstate-cli/src/repl.rs`:
   - In `run_repl_stream`: when `ReplControl::SubmitPrompt(prompt)` is returned, invoke `task_runner::run_task` or print execution notice/result using the session configuration.
4. Add tests in `crates/superstate-cli/tests/test_task_runner.rs`:
   - `test_task_config_from_cli`: asserts mapping from `Cli` to `TaskConfig` preserves all flags and defaults.
   - `test_run_task_missing_api_key_returns_clear_error`: unsets `GEMINI_API_KEY` and `GOOGLE_API_KEY`, runs `run_task`, and asserts error message explicitly mentions `GEMINI_API_KEY`.
5. Verify:
   `cargo test -p superstate --test test_task_runner`
   `cargo check --workspace --all-targets`
6. Commit:
   ```bash
   git add crates/superstate-cli/src/task_runner.rs crates/superstate-cli/src/lib.rs crates/superstate-cli/src/main.rs crates/superstate-cli/src/repl.rs crates/superstate-cli/tests/test_task_runner.rs
   git commit --no-gpg-sign -m "feat(cli): integrate headless single-shot solver and wire REPL prompt dispatch"
   ```
7. Run `roborev status; roborev show HEAD`.
8. Write report to `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness/docs/superpowers/plans/task-4-report.md`.
