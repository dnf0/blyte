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

/// Arguments for initializing state from a plan or direct goal.
#[derive(clap::Args, Clone, Debug)]
pub struct InitArgs {
    /// Path to plan markdown (`--from-plan` is accepted for reference parity)
    #[arg(short, long, alias = "from-plan")]
    pub plan: Option<PathBuf>,
    /// Path to state JSON file (`--out` is accepted for reference parity)
    #[arg(short, long, alias = "out", default_value = DEFAULT_STATE)]
    pub state: PathBuf,
    /// Disable git worktree creation; operate in-place in workspace root
    #[arg(long)]
    pub no_worktree: bool,
    /// Goal description for direct initialization without a plan
    #[arg(long)]
    pub goal: Option<String>,
    /// Verification command required when --goal is provided
    #[arg(long)]
    pub verify_cmd: Option<String>,
    /// Comma-separated list of files to modify
    #[arg(long)]
    pub files: Option<String>,
}

impl Default for InitArgs {
    fn default() -> Self {
        Self {
            plan: None,
            state: PathBuf::from(DEFAULT_STATE),
            no_worktree: false,
            goal: None,
            verify_cmd: None,
            files: None,
        }
    }
}

impl InitArgs {
    /// Helper to construct InitArgs from a plan path.
    pub fn for_plan(plan: PathBuf, state: PathBuf, no_worktree: bool) -> Self {
        Self {
            plan: Some(plan),
            state,
            no_worktree,
            goal: None,
            verify_cmd: None,
            files: None,
        }
    }
}

/// Arguments for showing execution state in markdown or JSON format.
#[derive(clap::Args, Clone, Debug)]
pub struct ShowArgs {
    #[arg(short, long, default_value = DEFAULT_STATE)]
    pub state: PathBuf,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub json: bool,
    /// Emit formatted markdown view
    #[arg(long)]
    pub markdown: bool,
}

impl Default for ShowArgs {
    fn default() -> Self {
        Self {
            state: PathBuf::from(DEFAULT_STATE),
            json: false,
            markdown: false,
        }
    }
}

#[derive(Subcommand)]
pub enum StateCommands {
    /// Initialize state from plan markdown or direct goal
    Init(InitArgs),
    /// Show execution state (markdown or JSON)
    Show(ShowArgs),
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
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Disable git worktree creation; operate in-place in workspace root
        #[arg(long)]
        no_worktree: bool,
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

/// Find the project's state, searching upward from the working directory.
///
/// A default relative path is resolved against the nearest ancestor that has one,
/// so running from a subdirectory acts on the project's state rather than failing
/// or creating a second one beside it.
fn resolve(state: PathBuf) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    superstate_core::state_machine::resolve_state_path(&state, &cwd)
}

/// Dispatch a `state` subcommand. Shared by both binaries.
pub fn dispatch(cmd: StateCommands) -> Result<(), String> {
    match cmd {
        StateCommands::Init(args) => commands::state::cmd_init(&args),
        StateCommands::Show(args) => {
            commands::state::cmd_show(&resolve(args.state), args.markdown, args.json)
        }
        StateCommands::Status { state, json } => commands::state::cmd_status(&resolve(state), json),
        StateCommands::Next {
            state,
            verify_cmd,
            verify_git_clean,
        } => commands::state::cmd_next(&resolve(state), verify_cmd, verify_git_clean),
        StateCommands::Ready { state, json } => commands::state::cmd_ready(&resolve(state), json),
        StateCommands::Checkpoint { state } => commands::state::cmd_checkpoint(&resolve(state)),
        StateCommands::Rollback { state, reference } => {
            commands::state::cmd_rollback(&resolve(state), reference.as_deref())
        }
        StateCommands::Prompt {
            state,
            task,
            with_graph,
            json,
            no_worktree,
        } => commands::state::cmd_prompt(
            &resolve(state),
            task.as_deref(),
            with_graph,
            json,
            no_worktree,
        ),
        StateCommands::Sync { plan, state, out } => {
            commands::state::cmd_sync(&resolve(state), &plan, out.as_deref())
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
            &resolve(state),
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
        } => commands::state::cmd_doctor(&resolve(state), plan.as_deref(), repair, json),
        StateCommands::Validate { state } => commands::state::cmd_validate(&resolve(state)),
        StateCommands::Apply {
            state,
            patch,
            note,
            out,
        } => commands::state::cmd_apply(
            &resolve(state),
            patch.as_deref(),
            note.as_deref(),
            out.as_deref(),
        ),
        StateCommands::Resume { state } => commands::state::cmd_resume(&resolve(state)),
        StateCommands::Metrics { state, json } => {
            commands::state::cmd_metrics(&resolve(state), json)
        }
        StateCommands::Decompose {
            plan,
            out,
            in_place,
        } => commands::state::cmd_decompose(&plan, out.as_deref(), in_place),
        StateCommands::Note {
            note,
            state,
            author,
        } => commands::state::cmd_note(&resolve(state), &note, &author),
    }
}
