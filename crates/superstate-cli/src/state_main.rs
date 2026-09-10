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

/// Restore the default SIGPIPE behaviour.
///
/// Rust disables SIGPIPE at start-up, so writing to a closed pipe returns EPIPE
/// and `println!` panics on it. That turns the ordinary `… | head` or quitting
/// out of a pager into a panic and a backtrace note. Restoring the default makes
/// the process exit quietly, as every other Unix command line tool does.
fn restore_sigpipe() {
    // Safety: setting a signal disposition to the default is always sound, and
    // this runs before any threads are spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn main() {
    restore_sigpipe();
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
