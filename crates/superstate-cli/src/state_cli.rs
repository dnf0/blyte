//! The `state` subcommand surface, defined once.
//!
//! Both binaries (`superstate state ...` and the standalone `state`) expose the
//! same commands. They previously each carried their own copy of this enum and
//! their own dispatch, so a command added to one compiled cleanly and then failed
//! at runtime with "unrecognized subcommand" when invoked through the other.

use clap::Subcommand;
use std::path::PathBuf;

use crate::commands;

const DEFAULT_STATE: &str = ".agent-state/state.json";

#[derive(Subcommand)]
pub enum StateCommands {
    /// Initialize state from plan markdown
    Init {
        /// Path to plan markdown (`--from-plan` is accepted for reference parity)
        #[arg(short, long, alias = "from-plan")]
        plan: PathBuf,
        /// Path to state JSON file (`--out` is accepted for reference parity)
        #[arg(short, long, alias = "out", default_value = DEFAULT_STATE)]
        state: PathBuf,
    },
    /// Display state status
    Status {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Advance active task with verification gates
    Next {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        #[arg(short = 'v', long)]
        verify_cmd: Option<String>,
        #[arg(long)]
        verify_git_clean: bool,
    },
    /// Display tasks whose dependencies are satisfied and may run now
    Ready {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        /// Emit a JSON array of task titles, for programmatic consumers
        #[arg(long)]
        json: bool,
    },
    /// Generate prompt for a task
    Prompt {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        /// Task to prompt for, by id or title. Defaults to the active task.
        #[arg(short, long)]
        task: Option<String>,
        /// Include knowledge-graph context in the prompt
        #[arg(long)]
        with_graph: bool,
    },
    /// Record note into epistemic ledger
    Note {
        note: String,
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        #[arg(short, long, default_value = "agent")]
        author: String,
    },
}

/// Dispatch a `state` subcommand. Shared by both binaries.
pub fn dispatch(cmd: StateCommands) -> Result<(), String> {
    match cmd {
        StateCommands::Init { plan, state } => commands::state::cmd_init(plan, state),
        StateCommands::Status { state, json } => commands::state::cmd_status(&state, json),
        StateCommands::Next {
            state,
            verify_cmd,
            verify_git_clean,
        } => commands::state::cmd_next(&state, verify_cmd, verify_git_clean),
        StateCommands::Ready { state, json } => commands::state::cmd_ready(&state, json),
        StateCommands::Prompt {
            state,
            task,
            with_graph,
        } => commands::state::cmd_prompt(&state, task.as_deref(), with_graph),
        StateCommands::Note {
            note,
            state,
            author,
        } => commands::state::cmd_note(&state, &note, &author),
    }
}
