use crate::commands;
use crate::state_cli::StateCommands;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, PartialEq)]
#[command(
    name = "superstate",
    version = "0.1.0",
    about = "Superstate: Autonomous agent harness with deterministic state and verification gates",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Direct task prompt to run in single-shot / headless mode
    #[arg(index = 1)]
    pub prompt: Option<String>,

    /// Verification gate command (e.g. 'pytest', 'cargo test')
    #[arg(short, long)]
    pub gate: Option<String>,

    /// Context strategy: 'state-only' (O(1) prompt, latest result only), 'state-window' (O(1) prompt plus the last 8 results) or 'conversational' (append-all chat) or 'folded' (append-only chat, oldest half folded into digests past a budget) or 'compact' (folded, plus superseded outputs masked and duplicates referenced)
    #[arg(
        long,
        default_value = "folded",
        value_parser = ["state-only", "state-window", "conversational", "folded", "compact"]
    )]
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

    /// Enable automatic rollback to baseline git commit if gate fails (disable with --rollback=false)
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub rollback: bool,

    /// Disable the loop-breaker guards (redundant read / inspection interception). Ablation control
    #[arg(long)]
    pub no_loop_breaker: bool,

    /// Working directory inside the container when --container is set
    #[arg(long, default_value = "/workspace")]
    pub container_workdir: String,

    /// Wall-clock budget in seconds; the run stops itself and reports when it is spent
    #[arg(long)]
    pub max_seconds: Option<u64>,

    /// Write a JSON run report to this path when the headless run exits
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Gate by autonomous TDD: derive a RED (failing reproduction test) then GREEN (fix) phase
    /// from the repository's own test runner. Fails closed on an unrecognised repository
    #[arg(long, conflicts_with = "gate")]
    pub tdd: bool,

    /// Before accepting a passing verification, review the change against the task with the same
    /// model; unmet requirements send the worker back (at most twice)
    #[arg(long)]
    pub review: bool,

    /// When a fold first appears, ask the worker for a structured note on the folded turns and
    /// carry it in the digest (folded and compact contexts)
    #[arg(long)]
    pub fold_notes: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
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
    Daemon(crate::state_cli::DaemonArgs),
    /// Cancel a running background daemon process
    Cancel(crate::state_cli::CancelArgs),
    /// Inspect background daemon logs
    Logs(crate::state_cli::LogsArgs),
    /// Prepare standardized cross-agent handoff package
    Handoff(crate::state_cli::HandoffArgs),
    /// Execute a shell command through the output shunting distillation layer
    Shunt {
        /// Line count threshold before shunting triggers
        #[arg(long, default_value = "50")]
        threshold_lines: usize,
        /// Directory where full unstripped command logs are stored
        #[arg(long)]
        logs_dir: Option<PathBuf>,
        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
    /// Inspect AST symbols, outlines, and code slices
    Inspect(commands::inspect::InspectArgs),
    /// Intercept and evaluate agent tool invocations before execution
    Guard {
        #[command(subcommand)]
        cmd: commands::guard::GuardCommands,
    },
}
