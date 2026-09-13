use crate::models::{ExecutionStatus, LedgerEntry};
use crate::prompt::{render_turnkey_prompt, resolve_project_root};

use crate::state_machine::{advance_step, load_state, save_state_atomic, StateError};
use crate::verifier::{run_verification_cmd, GateError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("State error: {0}")]
    State(#[from] StateError),
    #[error("Verification gate error: {0}")]
    Gate(#[from] GateError),
    #[error("Driver execution error: {0}")]
    Driver(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("No active task available to run")]
    NoActiveTask,
    #[error("Maximum retries ({0}) exceeded for task {1}")]
    MaxRetriesExceeded(usize, String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub state_path: PathBuf,
    pub driver_cmd: Option<String>,
    pub max_retries: usize,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonStepOutcome {
    Advanced {
        completed_task_id: String,
        next_task_id: Option<String>,
    },
    Failed {
        task_id: String,
        reason: String,
        retries_remaining: usize,
    },
    Blocked {
        task_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionReport {
    pub status: ExecutionStatus,
    pub milestones_completed: usize,
    pub base_branch: Option<String>,
    pub completion_file: PathBuf,
}

pub struct DaemonRunner {
    config: DaemonConfig,
    retry_counts: HashMap<String, usize>,
}

impl DaemonRunner {
    pub fn new(config: DaemonConfig) -> Result<Self, DaemonError> {
        Ok(Self {
            config,
            retry_counts: HashMap::new(),
        })
    }

    pub fn run_step(&mut self) -> Result<DaemonStepOutcome, DaemonError> {
        let mut state = load_state(&self.config.state_path)?;

        let active_task = match state.active_task.clone() {
            Some(t) => t,
            None => {
                if state.status == ExecutionStatus::Completed {
                    return Ok(DaemonStepOutcome::Advanced {
                        completed_task_id: "".to_string(),
                        next_task_id: None,
                    });
                }
                return Err(DaemonError::NoActiveTask);
            }
        };

        let working_dir = if let Some(ref wt) = state.active_worktree {
            wt.clone()
        } else {
            resolve_project_root(&self.config.state_path)
        };

        let prompt = render_turnkey_prompt(
            &state,
            &self.config.state_path,
            None,
            state.no_worktree,
            false,
        )
        .map_err(DaemonError::Driver)?;


        // Execute headless driver worker if provided
        if let Some(ref driver) = self.config.driver_cmd {
            let output = Command::new("sh")
                .arg("-c")
                .arg(driver)
                .current_dir(&working_dir)
                .env("SUPERSTATE_PROMPT", &prompt)
                .env("SUPERSTATE_ACTIVE_TASK", &active_task.id)
                .output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                return Err(DaemonError::Driver(format!(
                    "Driver command failed: {}",
                    stderr
                )));
            }
        }

        // Execute verification gate
        let verify_cmd = active_task
            .verification_cmd
            .as_deref()
            .unwrap_or("true");

        let gate_res = run_verification_cmd(verify_cmd, &working_dir, self.config.timeout_secs)?;

        if gate_res.passed {
            self.retry_counts.remove(&active_task.id);
            advance_step(&mut state)?;
            save_state_atomic(&self.config.state_path, &state)?;

            let next_task_id = state.active_task.as_ref().map(|t| t.id.clone());
            Ok(DaemonStepOutcome::Advanced {
                completed_task_id: active_task.id,
                next_task_id,
            })
        } else {
            let retries = self.retry_counts.entry(active_task.id.clone()).or_insert(0);
            *retries += 1;

            let fail_reason = gate_res
                .failure_reason
                .clone()
                .unwrap_or_else(|| gate_res.output.clone());

            state.notes.push(LedgerEntry::new(
                "superstate-daemon",
                format!("[verify-fail] Task {} failed: {}", active_task.id, fail_reason),
            ));

            if *retries >= self.config.max_retries {
                state.status = ExecutionStatus::Blocked;
                save_state_atomic(&self.config.state_path, &state)?;
                Ok(DaemonStepOutcome::Blocked {
                    task_id: active_task.id,
                    reason: format!("Exceeded maximum retries ({})", self.config.max_retries),
                })
            } else {
                save_state_atomic(&self.config.state_path, &state)?;
                Ok(DaemonStepOutcome::Failed {
                    task_id: active_task.id,
                    reason: fail_reason,
                    retries_remaining: self.config.max_retries - *retries,
                })
            }
        }
    }

    pub fn run_to_completion(&mut self) -> Result<CompletionReport, DaemonError> {
        loop {
            let state = load_state(&self.config.state_path)?;
            if state.status == ExecutionStatus::Completed || state.active_task.is_none() {
                break;
            }
            if state.status == ExecutionStatus::Blocked || state.status == ExecutionStatus::Failed {
                break;
            }

            match self.run_step()? {
                DaemonStepOutcome::Advanced { next_task_id, .. } => {
                    if next_task_id.is_none() {
                        break;
                    }
                }
                DaemonStepOutcome::Failed { .. } => {
                    continue;
                }
                DaemonStepOutcome::Blocked { task_id, reason } => {
                    return Err(DaemonError::MaxRetriesExceeded(
                        self.config.max_retries,
                        format!("{}: {}", task_id, reason),
                    ));
                }
            }
        }

        let final_state = load_state(&self.config.state_path)?;
        let completion_file = self
            .config
            .state_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("completion.json");

        let report = CompletionReport {
            status: final_state.status.clone(),
            milestones_completed: final_state.completed_tasks.len(),
            base_branch: final_state.base_branch.clone(),
            completion_file: completion_file.clone(),
        };

        let serialized = serde_json::to_string_pretty(&report)?;
        fs::write(&completion_file, serialized)?;

        Ok(report)
    }
}
