//! The epistemic ledger as an append-only file beside the state.

use blyte_core::ledger::{append_ledger_note, ledger_path, read_ledger};
use blyte_core::models::{ExecutionState, ExecutionStatus, LedgerEntry};
use blyte_core::state_machine::save_state_atomic;
use std::fs;

fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join(".agent-state").join("state.json");
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: None,
        pending_tasks: Vec::new(),
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: None,
        notes: Vec::new(),
        ..Default::default()
    };
    save_state_atomic(&state_path, &state).unwrap();
    (dir, state_path)
}

#[test]
fn notes_are_appended_to_the_ledger_not_the_state() {
    let (_d, state_path) = project();
    append_ledger_note(&state_path, LedgerEntry::new("agent", "a finding")).unwrap();

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(
        state["notes"].as_array().map(Vec::len),
        Some(0),
        "prose must not live in the document rewritten on every transition"
    );
    assert_eq!(read_ledger(&state_path).len(), 1);
}

#[test]
fn multiline_prose_survives_a_round_trip() {
    // Markdown with quotes and backticks is exactly what a milestone summary
    // looks like, and what JSON string escaping mangled.
    let (_d, state_path) = project();
    let note = "### Finding\n\nNeeds a `lookahead` token. See \"parser.go\".";
    append_ledger_note(&state_path, LedgerEntry::new("agent", note)).unwrap();

    assert_eq!(read_ledger(&state_path)[0].note, note);
}

#[test]
fn a_torn_final_line_costs_one_entry_not_the_ledger() {
    // An interrupted append should not make every earlier finding unreadable.
    let (_d, state_path) = project();
    for i in 0..3 {
        append_ledger_note(
            &state_path,
            LedgerEntry::new("agent", format!("note {}", i)),
        )
        .unwrap();
    }
    fs::write(
        ledger_path(&state_path),
        format!(
            "{}{}",
            fs::read_to_string(ledger_path(&state_path)).unwrap(),
            "{\"timestamp\":\"x\",\"author\":\"a\",\"note\":\"trunc"
        ),
    )
    .unwrap();

    assert_eq!(read_ledger(&state_path).len(), 3);
}

#[test]
fn appending_does_not_rewrite_earlier_entries() {
    // The whole point of the split: cost per append must not grow with the
    // number of findings already recorded.
    let (_d, state_path) = project();
    append_ledger_note(&state_path, LedgerEntry::new("agent", "first")).unwrap();
    let first_line = fs::read_to_string(ledger_path(&state_path)).unwrap();

    append_ledger_note(&state_path, LedgerEntry::new("agent", "second")).unwrap();
    let both = fs::read_to_string(ledger_path(&state_path)).unwrap();

    assert!(
        both.starts_with(&first_line),
        "the earlier entry was rewritten"
    );
}

#[test]
fn an_absent_ledger_reads_as_empty_rather_than_failing() {
    let (_d, state_path) = project();
    assert!(read_ledger(&state_path).is_empty());
}

#[test]
fn legacy_inline_notes_are_read_without_being_rewritten() {
    // States written before the split keep their notes inline. Reading them must
    // not mutate the document: a read that writes fails on a read-only checkout
    // and races a concurrent writer, for no benefit beyond tidiness.
    let (_d, state_path) = project();
    let mut state = blyte_core::state_machine::load_state(&state_path).unwrap();
    state.notes = vec![LedgerEntry::new("agent", "legacy finding")];
    save_state_atomic(&state_path, &state).unwrap();
    let before = fs::read_to_string(&state_path).unwrap();

    let entries = read_ledger(&state_path);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].note, "legacy finding");
    assert_eq!(
        fs::read_to_string(&state_path).unwrap(),
        before,
        "reading findings must not rewrite the state"
    );
}

#[test]
fn legacy_and_new_findings_are_returned_in_order() {
    let (_d, state_path) = project();
    let mut state = blyte_core::state_machine::load_state(&state_path).unwrap();
    state.notes = vec![LedgerEntry::new("agent", "older, held inline")];
    save_state_atomic(&state_path, &state).unwrap();

    append_ledger_note(
        &state_path,
        LedgerEntry::new("agent", "newer, in the ledger"),
    )
    .unwrap();

    let notes: Vec<String> = read_ledger(&state_path)
        .into_iter()
        .map(|e| e.note)
        .collect();
    assert_eq!(notes, vec!["older, held inline", "newer, in the ledger"]);
}
