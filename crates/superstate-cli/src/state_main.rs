//! Standalone `state` binary. The subcommand surface lives in `state_cli` so the
//! two binaries cannot drift apart.

use clap::Parser;
use std::path::PathBuf;
use superstate_cli::state_cli::{dispatch, StateCommands};

#[derive(Parser)]
#[command(
    name = "state",
    about = "Deterministic state machine operations for agent execution"
)]
struct StateCli {
    #[command(subcommand)]
    cmd: Option<StateCommands>,
}

fn main() {
    let cli = StateCli::parse();
    let cmd = cli.cmd.unwrap_or(StateCommands::Status {
        state: PathBuf::from(".agent-state/state.json"),
        json: false,
    });

    if let Err(err) = dispatch(cmd) {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
