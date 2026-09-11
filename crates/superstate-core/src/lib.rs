pub mod checkpoint;
pub mod ledger;
pub mod maintenance;
pub mod models;
pub mod patching;
pub mod plan_parser;
pub mod prompt;
pub mod state_machine;
pub mod verifier;

pub use ledger::append_ledger_note;
pub use models::{ExecutionState, ExecutionStatus, LedgerEntry, MilestoneSpec, TaskSpec};
pub use plan_parser::{init_state_from_plan, parse_plan_markdown, ParseError};
pub use prompt::{compact_ledger_note, render_turnkey_prompt, resolve_project_root, select_task};
pub use state_machine::{
    advance_step, branch_scoped_state_path, find_branch_state_for_worktree, init_state_direct,
    init_state_milestones, load_state, sanitize_branch_slug, save_state_atomic, StateError,
};
pub use verifier::{
    execute_all_gates, execute_all_gates_filtered, run_verification_cmd, verify_git_clean,
    verify_git_clean_filtered, GateError, GateResult,
};
