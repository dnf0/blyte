use crate::models::LedgerEntry;
use crate::state_machine::{load_state, save_state_atomic, StateError};
use std::path::Path;

pub fn append_ledger_note(state_path: &Path, entry: LedgerEntry) -> Result<(), StateError> {
    let mut state = load_state(state_path)?;
    state.notes.push(entry);
    save_state_atomic(state_path, &state)?;
    Ok(())
}
