//! A saved run: the history and state at the last checkpoint, enough to `--resume` it.

use std::path::Path;

use serde::{Deserialize, Serialize};
use superstate_agent::state::State;
use superstate_provider::Message;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: u32,
    pub state: State,
    pub history: Vec<Message>,
}

/// Write atomically (a temporary file, then rename), so a crash mid-write never leaves a
/// truncated journal in place of the last good one.
pub fn write(path: &Path, state: &State, history: &[Message]) -> Result<(), String> {
    let body = serde_json::to_vec(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "state": state,
        "history": history,
    }))
    .map_err(|e| format!("journal: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("journal {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("journal {}: {e}", path.display()))
}

pub fn read(path: &Path) -> Result<Journal, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("journal {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("journal {}: {e}", path.display()))
}
