use crate::models::{ExecutionState, ExecutionStatus, TaskSpec};
use crate::state_machine::{save_state_atomic, StateError};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Plan parsing error: {0}")]
    InvalidFormat(String),
}

impl From<ParseError> for StateError {
    fn from(err: ParseError) -> Self {
        StateError::InvalidTransition(err.to_string())
    }
}

pub fn parse_plan_markdown(content: &str) -> Result<Vec<TaskSpec>, ParseError> {
    let mut tasks = Vec::new();
    let mut current_task: Option<TaskSpec> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        // Two plan dialects are accepted: markdown headings ("### Task 1: Title")
        // and checkbox lists ("- [ ] Task 1: Title"). The reference implementation
        // parses both, and rejecting one meant a plan that worked there failed
        // here with "No tasks found".
        let heading = trimmed
            .strip_prefix("### Task ")
            .or_else(|| trimmed.strip_prefix("## Task "))
            .or_else(|| {
                trimmed
                    .strip_prefix("- [ ] Task ")
                    .or_else(|| trimmed.strip_prefix("- [x] Task "))
                    .or_else(|| trimmed.strip_prefix("* [ ] Task "))
            });

        if let Some(after_prefix) = heading {
            if let Some(task) = current_task.take() {
                tasks.push(task);
            }

            let after_prefix = after_prefix.trim();
            let (id, title) = if let Some((num_part, title_part)) = after_prefix.split_once(':') {
                (
                    format!("Task {}", num_part.trim()),
                    title_part.trim().to_string(),
                )
            } else {
                (format!("Task {}", after_prefix), after_prefix.to_string())
            };

            current_task = Some(TaskSpec {
                id,
                title,
                depends_on: Vec::new(),
                files_modify: Vec::new(),
                interfaces_produces: Vec::new(),
                verification_cmd: None,
            });
            continue;
        }

        if let Some(ref mut task) = current_task {
            if trimmed.starts_with("- **Depends on:**") || trimmed.starts_with("**Depends on:**") {
                let deps_str = if let Some(after) = trimmed.split_once("**Depends on:**") {
                    after.1.trim()
                } else {
                    ""
                };
                if deps_str != "none" && !deps_str.is_empty() {
                    for dep in deps_str.split(',') {
                        let clean_dep = dep.trim().trim_matches('`').trim();
                        if !clean_dep.is_empty() {
                            task.depends_on.push(clean_dep.to_string());
                        }
                    }
                }
            } else if trimmed.starts_with("- **Verification Command:**")
                || trimmed.starts_with("**Verification Command:**")
            {
                if let Some(after) = trimmed.split_once("**Verification Command:**") {
                    let cmd = after.1.trim().trim_matches('`').trim();
                    if !cmd.is_empty() {
                        task.verification_cmd = Some(cmd.to_string());
                    }
                }
            } else if trimmed.starts_with("- Create:")
                || trimmed.starts_with("- Modify:")
                || trimmed.starts_with("- Test:")
            {
                let file_part = trimmed
                    .split_once(':')
                    .map(|x| x.1.trim())
                    .unwrap_or("")
                    .trim_matches('`')
                    .trim();
                if !file_part.is_empty() {
                    task.files_modify.push(PathBuf::from(file_part));
                }
            }
        }
    }

    if let Some(task) = current_task {
        tasks.push(task);
    }

    if tasks.is_empty() {
        return Err(ParseError::InvalidFormat(
            "No tasks found starting with '### Task '".to_string(),
        ));
    }

    Ok(tasks)
}

pub fn init_state_from_plan(
    plan_path: &Path,
    state_path: &Path,
) -> Result<ExecutionState, StateError> {
    let content = fs::read_to_string(plan_path)?;
    let mut tasks = parse_plan_markdown(&content)?;

    let active_task = if !tasks.is_empty() {
        Some(tasks.remove(0))
    } else {
        None
    };

    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task,
        pending_tasks: tasks,
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: Some(plan_path.to_path_buf()),
        notes: Vec::new(),
    };

    save_state_atomic(state_path, &state)?;
    Ok(state)
}
