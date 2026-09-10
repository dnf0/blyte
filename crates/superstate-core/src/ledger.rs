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
//!
//! States written before the split kept their notes inline. Rather than migrating
//! them, readers accept both: `read_ledger` merges any inline notes with the file,
//! and every writer appends to the file. The old field is inert and stays where it
//! is. This avoids a migration that would have to mutate a state document during a
//! read, which would make `prompt` fail on a read-only checkout and race with a
//! concurrent writer, for no benefit beyond tidiness.

use crate::models::LedgerEntry;
use crate::state_machine::{load_state, StateError};
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

/// Read every recorded finding: inline notes first, then the ledger file.
///
/// Inline notes come from states written before the ledger was split out. They
/// are read but never written, so they stay in chronological order ahead of
/// anything appended since.
///
/// A line that cannot be parsed is skipped rather than failing the read: a torn
/// final line from an interrupted write should cost that entry and no more, not
/// every finding recorded before it.
pub fn read_ledger(state_path: &Path) -> Vec<LedgerEntry> {
    let mut entries = load_state(state_path)
        .map(|state| state.notes)
        .unwrap_or_default();

    if let Ok(file) = std::fs::File::open(ledger_path(state_path)) {
        entries.extend(
            BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str::<LedgerEntry>(&l).ok()),
        );
    }
    entries
}
