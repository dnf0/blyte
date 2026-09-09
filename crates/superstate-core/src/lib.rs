pub mod checkpoint;
pub mod ledger;
pub mod models;
pub mod plan_parser;
pub mod state_machine;
pub mod verifier;

pub use ledger::append_ledger_note;
pub use models::{ExecutionState, ExecutionStatus, LedgerEntry, TaskSpec};
pub use plan_parser::{init_state_from_plan, parse_plan_markdown, ParseError};
pub use state_machine::{advance_step, load_state, save_state_atomic, StateError};
pub use verifier::{
    execute_all_gates, run_verification_cmd, verify_git_clean, GateError, GateResult,
};
