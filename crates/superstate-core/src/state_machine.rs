use crate::models::{ExecutionState, ExecutionStatus, MilestoneSpec, TaskSpec};
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
        base_branch: None,
    }
}

/// Initializes execution state directly from a sequence of milestone specifications without a plan.md file.
///
/// Constructs a sequential DAG where the first milestone becomes `task-1` (the active task),
/// and subsequent milestones become `task-2..N` (pending tasks) with each task depending on the previous one.
///
/// # Arguments
/// * `goal` - Optional high-level goal description for context.
/// * `milestones` - Non-empty sequence of milestone specifications.
/// * `base_branch` - Optional name of the active feature branch to merge back into.
/// * `no_worktree` - Whether to disable worktree isolation and operate directly in the repository root.
///
/// # Returns
/// An `ExecutionState` initialized with status `InProgress`, step 1, `task-1` active, and pending tasks chained.
///
/// # Errors
/// Returns `StateError::InvalidTransition` if `milestones` is empty, or if any milestone has an empty title or verify command.
pub fn init_state_milestones(
    _goal: Option<&str>,
    milestones: Vec<MilestoneSpec>,
    base_branch: Option<&str>,
    no_worktree: bool,
) -> Result<ExecutionState, StateError> {
    // Fail fast if no milestones were provided, preventing an invalid state with no executable tasks
    if milestones.is_empty() {
        return Err(StateError::InvalidTransition(
            "milestones cannot be empty".to_string(),
        ));
    }

    // Validate that each milestone defines both a non-empty title and verification command
    for (idx, milestone) in milestones.iter().enumerate() {
        if milestone.title.trim().is_empty() {
            return Err(StateError::InvalidTransition(format!(
                "milestone {} title cannot be empty",
                idx + 1
            )));
        }
        if milestone.verify_cmd.trim().is_empty() {
            return Err(StateError::InvalidTransition(format!(
                "milestone {} verify_cmd cannot be empty",
                idx + 1
            )));
        }
    }

    // Deconstruct milestones into the initial active task (task-1) and pending sequential tasks (task-2..N)
    let mut iter = milestones.into_iter();
    // Guard against empty iterator; safely return an error rather than panicking if empty
    let Some(first) = iter.next() else {
        return Err(StateError::InvalidTransition(
            "milestones cannot be empty".to_string(),
        ));
    };

    let task_1 = TaskSpec {
        id: "task-1".to_string(),
        title: first.title,
        depends_on: Vec::new(),
        files_modify: first.files,
        interfaces_produces: Vec::new(),
        verification_cmd: Some(first.verify_cmd),
    };

    // Build subsequent tasks in the DAG, each chained to depend on the previous task ID
    let mut pending_tasks = Vec::new();
    for (i, milestone) in iter.enumerate() {
        let current_task_num = i + 2;
        let prev_task_num = i + 1;
        pending_tasks.push(TaskSpec {
            id: format!("task-{}", current_task_num),
            title: milestone.title,
            depends_on: vec![format!("task-{}", prev_task_num)],
            files_modify: milestone.files,
            interfaces_produces: Vec::new(),
            verification_cmd: Some(milestone.verify_cmd),
        });
    }

    // Assemble initial ExecutionState at step 1 in progress with configured base branch
    Ok(ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(task_1),
        pending_tasks,
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: None,
        notes: Vec::new(),
        active_worktree: None,
        active_branch: None,
        no_worktree,
        base_branch: base_branch.map(ToString::to_string),
    })
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

/// Sanitizes a git branch name into a safe filesystem slug.
///
/// Replaces characters outside `[a-zA-Z0-9._-]` with hyphens, collapses consecutive
/// hyphens into a single hyphen, and trims leading and trailing hyphens.
///
/// # Arguments
/// * `branch` - Name of the git branch.
///
/// # Returns
/// A sanitized string suitable for file naming.
pub fn sanitize_branch_slug(branch: &str) -> String {
    // Replace non-alphanumeric, non-dot, non-underscore, non-hyphen characters with hyphens
    let replaced: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Collapse consecutive hyphens into a single hyphen
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_hyphen = false;
    for c in replaced.chars() {
        if c == '-' {
            if !prev_hyphen {
                collapsed.push(c);
                prev_hyphen = true;
            }
        } else {
            collapsed.push(c);
            prev_hyphen = false;
        }
    }

    // Trim leading and trailing hyphens
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "branch".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Computes the state file path for a given branch in a repository.
///
/// Default branches (`main`, `master`) use `.agent-state/state.json` for 100% backward compatibility.
/// Feature branches route to `.agent-state/branches/<slug>.json`.
///
/// # Arguments
/// * `repo_root` - Root path of the repository.
/// * `branch` - Current or target branch name.
///
/// # Returns
/// The resolved `PathBuf` for the state file.
pub fn branch_scoped_state_path(repo_root: &Path, branch: &str) -> PathBuf {
    if branch == "main" || branch == "master" {
        repo_root.join(STATE_DIR).join("state.json")
    } else {
        let slug = sanitize_branch_slug(branch);
        repo_root
            .join(STATE_DIR)
            .join("branches")
            .join(format!("{}.json", slug))
    }
}

/// Finds the branch-scoped state file associated with a worktree.
///
/// Scans `.agent-state/branches/` for valid state files and matches either:
/// 1. An exact match against `active_worktree` (canonicalized or relative to `repo_root`).
/// 2. A match against `active_branch` if `worktree_branch` is provided.
///
/// # Arguments
/// * `repo_root` - Root path of the repository.
/// * `worktree_path` - Path of the worktree being inspected.
/// * `worktree_branch` - Optional branch checked out in the worktree.
///
/// # Returns
/// `Some(PathBuf)` pointing to the matching branch state file, or `None` if not found.
pub fn find_branch_state_for_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    worktree_branch: Option<&str>,
) -> Option<PathBuf> {
    let branches_dir = repo_root.join(STATE_DIR).join("branches");
    if !branches_dir.is_dir() {
        return None;
    }

    // Collect all candidate JSON state files deterministically sorted
    let entries = fs::read_dir(&branches_dir).ok()?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    files.sort();

    let canonical_target = fs::canonicalize(worktree_path).ok();
    let mut branch_match = None;

    for file in files {
        if let Ok(state) = load_state(&file) {
            // Check active_worktree match first
            if let Some(ref wt) = state.active_worktree {
                let matches_worktree = if let Some(target_canon) = &canonical_target {
                    if let Ok(wt_canon) = fs::canonicalize(wt) {
                        &wt_canon == target_canon
                    } else if let Ok(wt_canon) = fs::canonicalize(repo_root.join(wt)) {
                        &wt_canon == target_canon
                    } else {
                        wt == worktree_path || repo_root.join(wt) == worktree_path
                    }
                } else {
                    wt == worktree_path || repo_root.join(wt) == worktree_path
                };

                if matches_worktree {
                    return Some(file);
                }
            }

            // Fallback match by active_branch
            if let Some(target_branch) = worktree_branch {
                if state.active_branch.as_deref() == Some(target_branch) && branch_match.is_none() {
                    branch_match = Some(file);
                }
            }
        }
    }

    branch_match
}

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
/// Checks for either `.agent-state/state.json` or any `.json` file in `.agent-state/branches/`.
/// Used to refuse creating a nested second state inside an existing project.
pub fn enclosing_state_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let state_dir = current.join(STATE_DIR);
        if state_dir.join("state.json").exists() {
            return Some(current.to_path_buf());
        }
        let branches_dir = state_dir.join("branches");
        if branches_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&branches_dir) {
                let has_json = entries.filter_map(|e| e.ok()).any(|e| {
                    e.path().is_file()
                        && e.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                });
                if has_json {
                    return Some(current.to_path_buf());
                }
            }
        }
        dir = current.parent();
    }
    None
}
