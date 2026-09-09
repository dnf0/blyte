use superstate_cli::commands;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        Commands::State { cmd } => match cmd {
            StateCommands::Init { plan, state } => commands::state::cmd_init(plan, state),
            StateCommands::Status { state } => commands::state::cmd_status(&state),
            StateCommands::Next {
                state,
                verify_cmd,
                verify_git_clean,
            } => commands::state::cmd_next(&state, verify_cmd, verify_git_clean),
            StateCommands::Prompt { state } => commands::state::cmd_prompt(&state),
            StateCommands::Note {
                note,
                state,
                author,
            } => commands::state::cmd_note(&state, note, author),
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
