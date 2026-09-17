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
        /// Git commit range (e.g. HEAD~1..HEAD). If omitted, inspects uncommitted diff in working tree
        commit_range: Option<String>,
        /// Repository root path
        #[arg(short, long, default_value = ".")]
        repo: PathBuf,
        /// Reviewer driver command (e.g. "agy --model gemini-3.8-flash-low" or "claude")
        #[arg(short, long)]
        driver: Option<String>,
        /// Path to .agent-state/state.json (auto-detected if omitted)
        #[arg(long)]
        state_path: Option<PathBuf>,
        /// Record critical/high findings into the state epistemic ledger
        #[arg(long)]
        record: bool,
        /// Output full ReviewReport JSON to stdout
        #[arg(long)]
        json: bool,
        /// Exit with code 1 if blocking issues are present
        #[arg(long)]
        fail_on_blocking: bool,
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
    Daemon(superstate_cli::state_cli::DaemonArgs),
    /// Cancel a running background daemon process
    Cancel(superstate_cli::state_cli::CancelArgs),
    /// Inspect background daemon logs
    Logs(superstate_cli::state_cli::LogsArgs),
    /// Prepare standardized cross-agent handoff package
    Handoff(superstate_cli::state_cli::HandoffArgs),
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
        Commands::Review {
            commit_range,
            repo,
            driver,
            state_path,
            record,
            json,
            fail_on_blocking,
        } => commands::review::cmd_review(commands::review::ReviewArgs {
            commit_range,
            repo,
            driver,
            state_path,
            record,
            json,
            fail_on_blocking,
        }),
        Commands::Init {
            dest,
            language,
            strictness,
        } => commands::init::cmd_init_rules(&dest, &language, &strictness),
        Commands::State { cmd } => superstate_cli::state_cli::dispatch(cmd),
        Commands::Worktree { cmd } => commands::worktree::dispatch_worktree(cmd),
        Commands::Daemon(args) => match args.command {
            Some(superstate_cli::state_cli::DaemonSubcommands::List { repo, json }) => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let repo_root = repo.unwrap_or_else(|| {
                    let mut cur = Some(cwd.as_path());
                    while let Some(d) = cur {
                        if d.join(".agent-state").exists() || d.join(".git").exists() {
                            return d.to_path_buf();
                        }
                        cur = d.parent();
                    }
                    cwd
                });
                commands::daemon::cmd_daemon_list(&repo_root, json)
            }
            None => commands::daemon::cmd_daemon(
                &args.state,
                args.driver.as_deref(),
                args.detach,
                args.max_retries,
                args.timeout,
            ),
        },
        Commands::Cancel(args) => {
            commands::daemon::cmd_cancel(args.state.as_deref(), args.branch.as_deref(), args.all)
        }
        Commands::Logs(args) => {
            commands::daemon::cmd_logs(args.state.as_deref(), args.branch.as_deref(), args.tail)
                .map(|_| ())
        }
        Commands::Handoff(args) => commands::handoff::cmd_handoff(&args),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
