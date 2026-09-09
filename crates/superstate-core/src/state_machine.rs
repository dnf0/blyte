use crate::models::{ExecutionState, ExecutionStatus};
use fd_lock::RwLock;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StateError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization/deserialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("State machine invalid transition: {0}")]
    InvalidTransition(String),
}

pub fn load_state(path: &Path) -> Result<ExecutionState, StateError> {
    if !path.exists() {
        return Err(StateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("State file not found at {}", path.display()),
        )));
    }

    let file = File::open(path)?;
    let lock = RwLock::new(file);
    let read_guard = lock.read()?;
    let mut content = String::new();
    (&*read_guard).read_to_string(&mut content)?;

    let state: ExecutionState = serde_json::from_str(&content)?;
    Ok(state)
}

pub fn save_state_atomic(path: &Path, state: &ExecutionState) -> Result<(), StateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp_file_path = parent.join(format!(".state.{}.tmp", std::process::id()));

    {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_file_path)?;
        let mut lock = RwLock::new(file);
        let mut write_guard = lock.write()?;

        let serialized = serde_json::to_string_pretty(state)?;
        write_guard.write_all(serialized.as_bytes())?;
        write_guard.flush()?;
    }

    fs::rename(&temp_file_path, path)?;

    Ok(())
}

pub fn advance_step(state: &mut ExecutionState) -> Result<(), StateError> {
    if let Some(completed) = state.active_task.take() {
        state.completed_tasks.push(completed);
    }

    if !state.pending_tasks.is_empty() {
        state.active_task = Some(state.pending_tasks.remove(0));
        state.step += 1;
        state.status = ExecutionStatus::InProgress;
    } else {
        state.active_task = None;
        state.step += 1;
        state.status = ExecutionStatus::Completed;
    }

    Ok(())
}
