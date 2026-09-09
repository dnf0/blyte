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
    /// Create a git checkpoint guarding the current step
    Checkpoint {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
    },
    /// Reset the working tree and untracked files to a checkpoint
    Rollback {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        /// Checkpoint to restore. Defaults to the current step's checkpoint.
        #[arg(short = 'r', long)]
        reference: Option<String>,
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
    /// Reconcile state with a revised plan, preserving completed tasks
    Sync {
        #[arg(short, long, alias = "from-plan")]
        plan: PathBuf,
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        /// Write the reconciled state here instead of in place
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Run a verification command and atomically advance or record failure
    Exec {
        #[arg(short = 'v', long)]
        verify_cmd: String,
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        #[arg(long)]
        verify_git_clean: bool,
        /// Checkpoint before running, so a failure can be rolled back
        #[arg(long)]
        checkpoint: bool,
        /// Restore the checkpoint if verification fails
        #[arg(long)]
        rollback_on_fail: bool,
        /// Record the failure but do not advance on success
        #[arg(long)]
        no_advance: bool,
        #[arg(long)]
        json: bool,
    },
    /// Diagnose plan/state drift and optionally repair it
    Doctor {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        #[arg(short, long)]
        plan: Option<PathBuf>,
        #[arg(long)]
        repair: bool,
        #[arg(long)]
        json: bool,
    },
    /// Validate a state JSON file against the execution schema
    Validate {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
    },
    /// Apply a state patch (null deletes) and/or append a note
    Apply {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        #[arg(long)]
        patch: Option<PathBuf>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Executive briefing for resuming after context loss
    Resume {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
    },
    /// Task telemetry and efficiency metrics
    Metrics {
        #[arg(short, long, default_value = DEFAULT_STATE)]
        state: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Decompose coarse plan tasks into atomic child micro-tasks
    Decompose {
        #[arg(short, long)]
        plan: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        in_place: bool,
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
        StateCommands::Checkpoint { state } => commands::state::cmd_checkpoint(&state),
        StateCommands::Rollback { state, reference } => {
            commands::state::cmd_rollback(&state, reference.as_deref())
        }
        StateCommands::Prompt {
            state,
            task,
            with_graph,
        } => commands::state::cmd_prompt(&state, task.as_deref(), with_graph),
        StateCommands::Sync { plan, state, out } => {
            commands::state::cmd_sync(&state, &plan, out.as_deref())
        }
        StateCommands::Exec {
            verify_cmd,
            state,
            verify_git_clean,
            checkpoint,
            rollback_on_fail,
            no_advance,
            json,
        } => commands::state::cmd_exec(
            &state,
            &verify_cmd,
            verify_git_clean,
            checkpoint,
            rollback_on_fail,
            !no_advance,
            json,
        ),
        StateCommands::Doctor {
            state,
            plan,
            repair,
            json,
        } => commands::state::cmd_doctor(&state, plan.as_deref(), repair, json),
        StateCommands::Validate { state } => commands::state::cmd_validate(&state),
        StateCommands::Apply {
            state,
            patch,
            note,
            out,
        } => commands::state::cmd_apply(&state, patch.as_deref(), note.as_deref(), out.as_deref()),
        StateCommands::Resume { state } => commands::state::cmd_resume(&state),
        StateCommands::Metrics { state, json } => commands::state::cmd_metrics(&state, json),
        StateCommands::Decompose {
            plan,
            out,
            in_place,
        } => commands::state::cmd_decompose(&plan, out.as_deref(), in_place),
        StateCommands::Note {
            note,
            state,
            author,
        } => commands::state::cmd_note(&state, &note, &author),
    }
}
