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
pub use prompt::{compact_ledger_note, render_turnkey_prompt, select_task};
pub use state_machine::{
    advance_step, init_state_direct, init_state_milestones, load_state, save_state_atomic,
    StateError,
};
pub use verifier::{
    execute_all_gates, run_verification_cmd, verify_git_clean, GateError, GateResult,
};
