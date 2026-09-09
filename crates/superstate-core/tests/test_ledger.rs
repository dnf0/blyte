use superstate_core::ledger::append_ledger_note;
use superstate_core::models::{ExecutionState, ExecutionStatus, LedgerEntry};
use superstate_core::state_machine::{load_state, save_state_atomic};
use tempfile::tempdir;

#[test]
fn test_append_ledger_note() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join(".agent-state").join("state.json");

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let entry1 = LedgerEntry::new("agent", "[discovery] API returned 200 OK");
    append_ledger_note(&state_file, entry1).expect("Failed to append note");

    let entry2 = LedgerEntry::new("human", "[constraint] Do not touch legacy parser");
    append_ledger_note(&state_file, entry2).expect("Failed to append note 2");

    let loaded = load_state(&state_file).unwrap();
    assert_eq!(loaded.notes.len(), 2);
    assert_eq!(loaded.notes[0].note, "[discovery] API returned 200 OK");
    assert_eq!(
        loaded.notes[1].note,
        "[constraint] Do not touch legacy parser"
    );
    assert_eq!(loaded.notes[1].author, "human");
}
