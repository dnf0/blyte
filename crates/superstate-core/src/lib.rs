pub mod checkpoint;
pub mod daemon;
pub mod guard;
pub mod handoff;
pub mod impact;
pub mod ledger;
pub mod maintenance;
pub mod models;
pub mod patching;
pub mod plan_parser;
pub mod prompt;
pub mod shunt;
pub mod state_machine;
pub mod verifier;

pub use daemon::{
    finalize_completed_execution, CompletionReport, DaemonConfig, DaemonError, DaemonRunner,
    DaemonStepOutcome, StepAdvancer,
};
pub use guard::{evaluate_file_read_guard, FileReadGuardConfig, GuardDecision};
pub use handoff::{
    generate_handoff_markdown, generate_resumption_command, write_handoff_package, HandoffSpec,
};
pub use ledger::append_ledger_note;
pub use models::{ExecutionState, ExecutionStatus, LedgerEntry, MilestoneSpec, TaskSpec};
pub use plan_parser::{init_state_from_plan, parse_plan_markdown, ParseError};
pub use prompt::{
    compact_ledger_note, format_retry_failure_section, render_turnkey_prompt, resolve_project_root,
    select_task,
};
pub use shunt::{evaluate_and_distill, prune_old_logs, DistilledOutput, ShuntConfig};
pub use state_machine::{
    advance_step, branch_scoped_state_path, find_branch_state_for_worktree, init_state_direct,
    init_state_milestones, load_state, sanitize_branch_slug, save_state_atomic, StateError,
};
pub use verifier::{
    execute_all_gates, execute_all_gates_filtered, run_verification_cmd, verify_git_clean,
    verify_git_clean_filtered, GateError, GateResult,
};
