//! The epistemic ledger: an append-only record of what earlier work discovered.
//!
//! Kept beside the state rather than inside it. The two have opposite shapes: the
//! state is small, bounded, and rewritten atomically on every transition, whereas
//! the ledger is prose, grows without bound, and is only ever appended to.
//!
//! Storing prose inside the state document cost more than it looked:
//!
//! * appending one note deserialised, mutated and rewrote the whole document, so
//!   the cost of recording a finding grew with the number of findings already
//!   recorded, and a crash mid-write risked the state as well as the note;
//! * JSON string escaping turned markdown into a single unreadable line, in the
//!   file a human opens precisely when a run has gone wrong;
//! * one appended note showed in a diff as one enormous changed line.
//!
//! One JSON object per line fixes all three. Appends are a single write with no
//! read-modify-write, a torn append loses one line instead of the state, and the
//! file stays machine readable.

use crate::models::LedgerEntry;
use crate::state_machine::{load_state, save_state_atomic, StateError};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Ledger file name, kept alongside `state.json`.
pub const LEDGER_FILE: &str = "ledger.jsonl";

/// The ledger belonging to a given state file.
pub fn ledger_path(state_path: &Path) -> PathBuf {
    state_path
        .parent()
        .map(|dir| dir.join(LEDGER_FILE))
        .unwrap_or_else(|| PathBuf::from(LEDGER_FILE))
}

/// Append one entry. A single write, whatever the ledger already holds.
pub fn append_ledger_note(state_path: &Path, entry: LedgerEntry) -> Result<(), StateError> {
    // Confirm the state exists first, so a note cannot be filed against a project
    // that has not been initialised.
    load_state(state_path)?;

    let path = ledger_path(state_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(&entry)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

/// Read the ledger, skipping any line that cannot be parsed.
///
/// A torn final line from an interrupted write should cost that entry and no
/// more; refusing to read the whole ledger over one bad line would lose every
/// finding recorded before it.
pub fn read_ledger(state_path: &Path) -> Vec<LedgerEntry> {
    let path = ledger_path(state_path);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<LedgerEntry>(&l).ok())
        .collect()
}

/// Move notes held inside an existing state document into the ledger file.
///
/// Returns how many were migrated. Idempotent: a state with no inline notes is
/// left alone, so this can run on every load without a version flag.
pub fn migrate_inline_notes(state_path: &Path) -> Result<usize, StateError> {
    let mut state = load_state(state_path)?;
    if state.notes.is_empty() {
        return Ok(0);
    }
    let migrated = state.notes.len();
    let path = ledger_path(state_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    for entry in &state.notes {
        writeln!(file, "{}", serde_json::to_string(entry)?)?;
    }
    state.notes.clear();
    save_state_atomic(state_path, &state)?;
    Ok(migrated)
}
