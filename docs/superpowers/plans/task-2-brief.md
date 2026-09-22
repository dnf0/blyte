# Task 2: CLI Argument Parser with Dual-Mode Support

## Goal
Implement dual-mode CLI argument parsing for `superstate`:
- Running `superstate` with no arguments defaults to interactive mode (`prompt == None`, `command == None`).
- Running `superstate "<prompt>"` or `superstate "<prompt>" --gate "pytest"` parses single-shot task mode.
- Running `superstate doctor`, `superstate review`, etc., parses subcommands cleanly.
- Support execution flags: `--gate`, `--context`, `--model`, `--container`, `--max-turns`, `--max-gates`, `--rollback`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## Files
- Create: `crates/superstate-cli/src/cli.rs` (defining `Cli` and `Commands`, derived with `clap::Parser` and `clap::Subcommand`, with `Debug, PartialEq`)
- Modify: `crates/superstate-cli/src/lib.rs` (add `pub mod cli;`)
- Modify: `crates/superstate-cli/src/main.rs` (use `superstate_cli::cli::{Cli, Commands}`)
- Create: `crates/superstate-cli/tests/test_cli_dispatch.rs`

## Detailed Implementation Steps
1. Create `crates/superstate-cli/src/cli.rs`:
   Move the `Cli` struct and `Commands` enum from `main.rs` into `cli.rs`:
   ```rust
   use clap::{Parser, Subcommand};
   use std::path::PathBuf;
   use crate::commands;
   use crate::state_cli::StateCommands;

   #[derive(Parser, Debug, PartialEq)]
   #[command(
       name = "superstate",
       version = "0.1.0",
       about = "Superstate: Autonomous agent harness with deterministic state and verification gates"
   )]
   pub struct Cli {
       /// Direct task prompt to run in single-shot / headless mode
       #[arg(index = 1)]
       pub prompt: Option<String>,

       /// Verification gate command (e.g. 'pytest', 'cargo test')
       #[arg(short, long)]
       pub gate: Option<String>,

       /// Context strategy: 'state-only' (SKILL.state O(1) prompt) or 'conversational' (append-all chat)
       #[arg(long, default_value = "conversational")]
       pub context: String,

       /// Model identifier or provider selection
       #[arg(short, long)]
       pub model: Option<String>,

       /// Run inside an isolated container workspace instead of local host
       #[arg(long)]
       pub container: Option<String>,

       /// Maximum turn budget
       #[arg(long, default_value = "25")]
       pub max_turns: usize,

       /// Maximum verification gate attempts before failing
       #[arg(long, default_value = "5")]
       pub max_gates: usize,

       /// Enable automatic rollback to baseline git commit if gate fails
       #[arg(long, default_value_t = true)]
       pub rollback: bool,

       #[command(subcommand)]
       pub command: Option<Commands>,
   }

   #[derive(Subcommand, Debug, PartialEq)]
   pub enum Commands {
       Doctor,
       Mcp,
       Review {
           commit_range: Option<String>,
           #[arg(short, long, default_value = ".")]
           repo: PathBuf,
           #[arg(short, long)]
           driver: Option<String>,
           #[arg(long)]
           state_path: Option<PathBuf>,
           #[arg(long)]
           record: bool,
           #[arg(long)]
           json: bool,
           #[arg(long)]
           fail_on_blocking: bool,
       },
       Init {
           #[arg(short, long, default_value = ".")]
           dest: PathBuf,
           #[arg(short, long, default_value = "rust")]
           language: String,
           #[arg(short, long, default_value = "balanced")]
           strictness: String,
       },
       State {
           #[command(subcommand)]
           cmd: StateCommands,
       },
       Worktree {
           #[command(subcommand)]
           cmd: commands::worktree::WorktreeCommands,
       },
       Daemon(crate::state_cli::DaemonArgs),
       Cancel(crate::state_cli::CancelArgs),
       Logs(crate::state_cli::LogsArgs),
       Handoff(crate::state_cli::HandoffArgs),
       Shunt {
           #[arg(long, default_value = "50")]
           threshold_lines: usize,
           #[arg(long)]
           logs_dir: Option<PathBuf>,
           #[arg(trailing_var_arg = true, required = true)]
           cmd: Vec<String>,
       },
       Inspect(commands::inspect::InspectArgs),
       Guard {
           #[command(subcommand)]
           cmd: commands::guard::GuardCommands,
       },
   }
   ```
2. Update `crates/superstate-cli/src/lib.rs` to expose `pub mod cli;`.
3. Update `crates/superstate-cli/src/main.rs` to use `superstate_cli::cli::{Cli, Commands}`.
   In `main()`:
   - If `cli.command` is `Some(cmd)`: match and execute existing subcommand logic.
   - If `cli.command` is `None`:
     - If `let Some(ref prompt) = cli.prompt`: for now, print `eprintln!("Running headless task: {prompt}");`
     - Else: print `eprintln!("Launching interactive REPL...");`
4. Add unit / integration tests in `crates/superstate-cli/tests/test_cli_dispatch.rs`:
   - `test_parse_no_args_launches_interactive`:
     Verifies parsing `["superstate"]` yields `prompt == None` and `command == None`.
   - `test_parse_single_shot_prompt`:
     Verifies parsing `["superstate", "fix issue", "--gate", "pytest", "--context", "state-only"]` yields `prompt == Some("fix issue")`, `gate == Some("pytest")`, `context == "state-only"`, `command == None`.
   - `test_parse_subcommand`:
     Verifies parsing `["superstate", "doctor"]` yields `command == Some(Commands::Doctor)` and `prompt == None`.
5. Run tests:
   `cargo test -p superstate --test test_cli_dispatch`
6. Run `cargo check --workspace --all-targets` to ensure 0 warnings.
7. Commit:
   ```bash
   git add crates/superstate-cli/src/cli.rs crates/superstate-cli/src/lib.rs crates/superstate-cli/src/main.rs crates/superstate-cli/tests/test_cli_dispatch.rs
   git commit --no-gpg-sign -m "feat(cli): add dual-mode prompt and execution flags to CLI parser"
   ```
8. Run `roborev status; roborev show HEAD`.
9. Write report to `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness/docs/superpowers/plans/task-2-report.md`.
