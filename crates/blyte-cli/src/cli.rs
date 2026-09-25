use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, PartialEq)]
#[command(
    name = "blyte",
    version = "0.1.0",
    about = "Blyte: Autonomous agent harness with deterministic state and verification gates"
)]
pub struct Cli {
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

    /// Write a JSON run report to this path when the headless run exits
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Approximate token budget for the verbatim tail before folding (folded context; default 48000)
    #[arg(long)]
    pub fold_budget: Option<usize>,
}
