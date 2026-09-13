use superstate_cli::commands;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use superstate_cli::state_cli::StateCommands;

#[derive(Parser)]
#[command(
    name = "superstate",
    version = "0.1.0",
    about = "Superstate: Deterministic state engine, AST intelligence, zero-noise review & MCP server"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run 5-step verification handshake and health check
    Doctor,
    /// Run stdio JSON-RPC MCP server
    Mcp,
    /// Adversarial code review over git diff
    Review {
        /// Git commit range (e.g. HEAD~1..HEAD)
        commit_range: Option<String>,
        /// Repository root path
        #[arg(short, long, default_value = ".")]
        repo: PathBuf,
    },
    /// Initialize agent guidance (AGENTS.md, Cursor rules)
    Init {
        /// Destination directory
        #[arg(short, long, default_value = ".")]
        dest: PathBuf,
        /// Primary language
        #[arg(short, long, default_value = "rust")]
        language: String,
        /// Strictness profile
        #[arg(short, long, default_value = "balanced")]
        strictness: String,
    },
    /// State machine operations
    State {
        #[command(subcommand)]
        cmd: StateCommands,
    },
    /// Git worktree and COW cache operations
    Worktree {
        #[command(subcommand)]
        cmd: commands::worktree::WorktreeCommands,
    },
    /// Run autonomous background daemon over milestones
    Daemon {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
        /// Headless coding driver (e.g. claude, agy, or command)
        #[arg(long)]
        driver: Option<String>,
        /// Detach execution into a background daemon process
        #[arg(long)]
        detach: bool,
        /// Maximum verification retries per milestone
        #[arg(long, default_value_t = 3)]
        max_retries: usize,
        /// Timeout in seconds per verification attempt
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Cancel a running background daemon process
    Cancel {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
    },
    /// Inspect background daemon logs
    Logs {
        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
        /// Number of lines to tail
        #[arg(short, long)]
        tail: Option<usize>,
    },
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
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Doctor => commands::doctor::run_doctor(),
        Commands::Mcp => superstate_mcp::server::run_stdio_mcp_server().map_err(|e| e.to_string()),
        Commands::Review { commit_range, repo } => {
            commands::review::cmd_review(&repo, commit_range)
        }
        Commands::Init {
            dest,
            language,
            strictness,
        } => commands::init::cmd_init_rules(&dest, &language, &strictness),
        Commands::State { cmd } => superstate_cli::state_cli::dispatch(cmd),
        Commands::Worktree { cmd } => commands::worktree::dispatch_worktree(cmd),
        Commands::Daemon {
            state,
            driver,
            detach,
            max_retries,
            timeout,
        } => commands::daemon::cmd_daemon(&state, driver.as_deref(), detach, max_retries, timeout),
        Commands::Cancel { state } => commands::daemon::cmd_cancel(&state),
        Commands::Logs { state, tail } => commands::daemon::cmd_logs(&state, tail).map(|_| ()),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
