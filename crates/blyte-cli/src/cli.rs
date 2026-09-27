use clap::Parser;
use std::path::PathBuf;

#[derive(clap::Subcommand, Debug, PartialEq, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
    Dispatch(DispatchArgs),
    /// Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
    Review(ReviewArgs),
    /// Install or print the Claude Code skill for the plan, delegate, review workflow
    Skill(SkillArgs),
}

#[derive(clap::Args, Debug, PartialEq, Clone)]
pub struct DispatchArgs {
    /// Identifier for this dispatch (used for worktree and branch wip/<id>)
    pub id: String,

    /// Path to the task description markdown file
    pub task_file: PathBuf,

    /// The check command; several lines are several steps, all of which must pass
    #[arg(short, long)]
    pub gate: String,

    /// The commit a new worktree starts from
    #[arg(long, default_value = "HEAD")]
    pub base: String,

    /// Model identifier or provider selection
    #[arg(short, long)]
    pub model: Option<String>,

    /// Wall-clock budget in seconds
    #[arg(long, default_value = "3600")]
    pub max_seconds: u64,

    /// Maximum turn budget
    #[arg(long, default_value = "300")]
    pub max_turns: usize,

    /// Maximum spend in USD
    #[arg(long = "max-cost")]
    pub max_cost_usd: Option<f64>,

    /// Approximate token budget for the verbatim tail before folding
    #[arg(long)]
    pub fold_budget: Option<usize>,

    /// Sandbox local command execution: 'on' or 'off' (default 'on')
    #[arg(
        long,
        default_value = "on",
        value_parser = ["on", "off"]
    )]
    pub sandbox: String,

    /// Outbound network mode: 'on' or 'off' (default 'on')
    #[arg(
        long,
        default_value = "on",
        value_parser = ["on", "off"]
    )]
    pub network: String,

    /// Let sandboxed commands read this path despite credential deny-list (repeatable)
    #[arg(long = "allow-read")]
    pub allow_read: Vec<PathBuf>,

    /// Pass this environment variable to commands (repeatable)
    #[arg(long = "pass-env")]
    pub pass_env: Vec<String>,
}

#[derive(clap::Args, Debug, PartialEq, Clone)]
pub struct ReviewArgs {
    pub id: String,

    #[arg(long)]
    pub diff: bool,
}

#[derive(clap::Subcommand, Debug, PartialEq, Clone)]
pub enum SkillAction {
    /// Install the Claude Code skill
    Install {
        /// Install to repository instead of personal ~/.claude
        #[arg(long)]
        project: bool,

        /// Overwrite even if existing skill was modified
        #[arg(long)]
        force: bool,

        /// Also tell Claude Code to delegate implementation to blyte by default, in CLAUDE.md
        #[arg(long, hide = true)]
        default: bool,

        /// Install the skill without making blyte the default in CLAUDE.md
        #[arg(long)]
        no_default: bool,
    },

    /// Remove the skill and the CLAUDE.md block
    Uninstall {
        /// Remove from repository instead of personal ~/.claude
        #[arg(long)]
        project: bool,
    },

    /// Print the skill markdown to stdout
    Print,
}

#[derive(clap::Args, Debug, PartialEq, Clone)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub action: SkillAction,
}

#[derive(Parser, Debug, PartialEq)]
#[command(
    name = "blyte",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("BLYTE_BUILD"), ")"),
    about = "A lean coding-agent harness with a sealed verification gate",
    subcommand_precedence_over_arg = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Direct task prompt to run in single-shot / headless mode
    #[arg(index = 1)]
    pub prompt: Option<String>,

    /// Verification gate command (e.g. 'pytest', 'cargo test'). Several steps may be given one per line: every step runs and all failures are reported together.
    #[arg(short, long)]
    pub gate: Option<String>,

    /// Context strategy: 'conversational' (append-all chat) or 'folded' (append-only chat, oldest half folded into digests past a budget)
    #[arg(
        long,
        default_value = "folded",
        value_parser = ["conversational", "folded"]
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

    /// Working directory inside the container when --container is set
    #[arg(long, default_value = "/workspace")]
    pub container_workdir: String,

    /// Wall-clock budget in seconds; the run stops itself and reports when it is spent
    #[arg(long)]
    pub max_seconds: Option<u64>,

    /// Maximum spend in USD; the run stops before a model call once reached
    #[arg(long = "max-cost")]
    pub max_cost_usd: Option<f64>,

    /// Write a JSON run report to this path when the headless run exits
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Approximate token budget for the verbatim tail before folding (folded context; default 48000)
    #[arg(long)]
    pub fold_budget: Option<usize>,

    /// Sandbox the commands blyte runs on this machine (macOS Seatbelt, Linux Landlock): 'on' or 'off'
    #[arg(
        long,
        default_value = "on",
        value_parser = ["on", "off"]
    )]
    pub sandbox: String,

    /// Let commands blyte runs connect out: 'on' or 'off' (default 'on'). 'off' needs the sandbox
    #[arg(
        long,
        default_value = "on",
        value_parser = ["on", "off"]
    )]
    pub network: String,

    /// Let sandboxed commands read this path despite the credential deny-list (repeatable)
    #[arg(long = "allow-read")]
    pub allow_read: Vec<PathBuf>,

    /// Pass this environment variable to commands even though it looks like a credential (repeatable)
    #[arg(long = "pass-env")]
    pub pass_env: Vec<String>,
}
