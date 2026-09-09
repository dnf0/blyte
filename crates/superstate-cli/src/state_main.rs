use superstate_cli::commands;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "state",
    version = "0.1.0",
    about = "Superstate state engine CLI"
)]
struct StateCli {
    #[command(subcommand)]
    cmd: Option<StateCommands>,
}

#[derive(Subcommand)]
enum StateCommands {
    /// Initialize state from plan markdown
    Init {
        /// Path to plan markdown
        #[arg(short, long)]
        plan: PathBuf,
        /// Path to state JSON file
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
    },
    /// Display state status
    Status {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
    },
    /// Advance active task with verification gates
    Next {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
        #[arg(short = 'v', long)]
        verify_cmd: Option<String>,
        #[arg(long)]
        verify_git_clean: bool,
    },
    /// Display tasks whose dependencies are satisfied and may run now
    Ready {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
        /// Emit a JSON array of task titles, for programmatic consumers
        #[arg(long)]
        json: bool,
    },
    /// Generate prompt for active task
    Prompt {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
    },
    /// Record note into epistemic ledger
    Note {
        note: String,
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
        #[arg(short, long, default_value = "agent")]
        author: String,
    },
}

fn main() {
    let cli = StateCli::parse();

    let cmd = cli.cmd.unwrap_or(StateCommands::Status {
        state: PathBuf::from(".agent-state/state.json"),
    });

    let result = match cmd {
        StateCommands::Init { plan, state } => commands::state::cmd_init(plan, state),
        StateCommands::Status { state } => commands::state::cmd_status(&state),
        StateCommands::Next {
            state,
            verify_cmd,
            verify_git_clean,
        } => commands::state::cmd_next(&state, verify_cmd, verify_git_clean),
        StateCommands::Ready { state, json } => commands::state::cmd_ready(&state, json),
        StateCommands::Prompt { state } => commands::state::cmd_prompt(&state),
        StateCommands::Note {
            note,
            state,
            author,
        } => commands::state::cmd_note(&state, note, author),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
