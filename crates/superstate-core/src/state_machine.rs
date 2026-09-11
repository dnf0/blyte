use crate::models::{ExecutionState, ExecutionStatus, TaskSpec};
use fd_lock::RwLock;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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

/// Initializes execution state directly from a goal and verification command without a plan.md file.
///
/// # Arguments
/// * `goal` - Concise description of the goal / active task title.
/// * `verify_cmd` - Shell verification command that must pass.
/// * `files_modify` - List of target files the task intends to modify.
/// * `no_worktree` - Whether to disable worktree provisioning and operate in-place.
///
/// # Returns
/// An `ExecutionState` initialized with status `InProgress`, step 1, and `task-1` as the active task.
pub fn init_state_direct(
    goal: &str,
    verify_cmd: &str,
    files_modify: Vec<PathBuf>,
    no_worktree: bool,
) -> ExecutionState {
    // Construct the primary active task directly from the provided goal and verification command
    let task = TaskSpec {
        id: "task-1".to_string(),
        title: goal.to_string(),
        depends_on: Vec::new(),
        files_modify,
        interfaces_produces: Vec::new(),
        verification_cmd: Some(verify_cmd.to_string()),
    };

    // Construct the initial in-progress state at step 1 without an associated plan path
    ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task),
        pending_tasks: Vec::new(),
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: None,
        notes: Vec::new(),
        active_worktree: None,
        active_branch: None,
        no_worktree,
    }
}

/// Advances execution state to the next task, completing the current active task.
///
/// Resets active worktree and branch tracking fields to None so that subsequent tasks
/// start with clean worktree state unless explicitly provisioned.
pub fn advance_step(state: &mut ExecutionState) -> Result<(), StateError> {
    if let Some(completed) = state.active_task.take() {
        state.completed_tasks.push(completed);
    }

    // Reset active worktree and branch tracking for the completed task
    state.active_worktree = None;
    state.active_branch = None;

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

/// Tasks whose dependencies are all satisfied, and which may therefore run now.
///
/// A pending task is unblocked when every entry in `depends_on` names a completed
/// task. Dependencies are written as they appear in the plan (`**Depends on:** Task 1`),
/// so they are matched against both id and title, and the conventional `none`
/// placeholder is treated as no dependency at all.
///
/// This is what makes the plan a DAG rather than a list: without it a runner can
/// only walk milestones in file order, which is not the same thing.
pub fn ready_tasks(state: &ExecutionState) -> Vec<&TaskSpec> {
    let completed: Vec<&str> = state
        .completed_tasks
        .iter()
        .flat_map(|t| [t.id.as_str(), t.title.as_str()])
        .collect();

    let is_satisfied = |dep: &String| -> bool {
        let dep = dep.trim();
        if dep.is_empty() || dep.eq_ignore_ascii_case("none") {
            return true;
        }
        completed
            .iter()
            .any(|done| done.eq_ignore_ascii_case(dep) || dep.starts_with(*done))
    };

    let mut ready: Vec<&TaskSpec> = Vec::new();
    if let Some(active) = state.active_task.as_ref() {
        if active.depends_on.iter().all(is_satisfied) {
            ready.push(active);
        }
    }
    for task in state.pending_tasks.iter() {
        if task.depends_on.iter().all(is_satisfied) {
            ready.push(task);
        }
    }
    ready
}

/// Directory holding a project's execution state.
pub const STATE_DIR: &str = ".agent-state";

/// Resolve a state path by searching upward for the project's state, as git does
/// for `.git`.
///
/// Without this an agent that changes into a subdirectory either loses the state
/// or, worse, creates a second one there: two states for one project, diverging
/// silently. Resolution rules:
///
/// * an absolute path, or a relative path that exists as given, is used as-is;
/// * otherwise the same relative path is sought in each ancestor of `start`,
///   and the first hit wins;
/// * if none is found the original is returned, so `init` still creates state in
///   the current directory.
pub fn resolve_state_path(path: &Path, start: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        return path.to_path_buf();
    }
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(path);
        if candidate.exists() {
            return candidate;
        }
        dir = current.parent();
    }
    path.to_path_buf()
}

/// The nearest ancestor of `start` that already holds project state, if any.
///
/// Used to refuse creating a nested second state inside an existing project.
pub fn enclosing_state_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        if current.join(STATE_DIR).join("state.json").exists() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}
