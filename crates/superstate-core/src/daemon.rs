use crate::models::{ExecutionStatus, LedgerEntry};
use crate::prompt::{format_retry_failure_section, render_turnkey_prompt, resolve_project_root};
use crate::state_machine::{
    advance_step, load_state, sanitize_branch_slug, save_state_atomic, StateError, STATE_DIR,
};
use crate::verifier::{run_verification_cmd, GateError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;

/// Closure callback hook for pluggable step advancement across workspace and worktree lifecycles.
pub type StepAdvancer = Box<dyn FnMut(&Path) -> Result<(), String> + Send + 'static>;

#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("State error: {0}")]
    State(#[from] StateError),
    #[error("Verification gate error: {0}")]
    Gate(#[from] GateError),
    #[error("Driver execution error: {0}")]
    Driver(String),
    #[error("Step advancer error: {0}")]
    StepAdvancer(String),
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

/// Resolves the job slug and the base `.agent-state` directory for a given state path.
///
/// Default/trunk state paths (`state.json`) map to slug `"default"`.
/// Feature branch states (`branches/<slug>.json`) map to their branch slug.
///
/// # Arguments
/// * `state_path` - Path to state JSON file.
///
/// # Returns
/// A tuple containing `(slug, agent_state_dir)`.
pub fn resolve_daemon_slug_and_state_dir(state_path: &Path) -> (String, PathBuf) {
    let slug = match state_path.file_stem().and_then(|s| s.to_str()) {
        Some("state") | None => "default".to_string(),
        Some(other) => sanitize_branch_slug(other),
    };

    // Locate .agent-state directory if present in ancestors
    let mut dir = state_path.parent();
    let mut agent_state_dir = None;
    while let Some(current) = dir {
        if current.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
            agent_state_dir = Some(current.to_path_buf());
            break;
        }
        dir = current.parent();
    }

    let base_dir = agent_state_dir.unwrap_or_else(|| {
        state_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });

    (slug, base_dir)
}

/// Scoped filesystem paths allocated to a daemon executing over a discrete state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonPaths {
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
    pub prompt_file: PathBuf,
    pub slug: String,
}

/// Resolves scoped daemon paths for a given state machine path.
///
/// Default state (`state.json`) maps to slug `"default"`, PID `.agent-state/daemons/default.pid`,
/// logs `.agent-state/logs/default.log`, and prompts `.agent-state/prompts/default.md`.
/// Feature branches (`branches/<slug>.json`) scope to `.agent-state/daemons/<slug>.pid`.
pub fn daemon_paths_for_state(state_path: &Path) -> DaemonPaths {
    let (slug, base_dir) = resolve_daemon_slug_and_state_dir(state_path);
    let state_dir = if base_dir.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
        base_dir
    } else {
        base_dir.join(STATE_DIR)
    };
    DaemonPaths {
        pid_file: state_dir.join("daemons").join(format!("{}.pid", slug)),
        log_file: state_dir.join("logs").join(format!("{}.log", slug)),
        prompt_file: state_dir.join("prompts").join(format!("{}.md", slug)),
        slug,
    }
}

/// Checks whether an operating system PID is active and alive.
#[cfg(unix)]
pub fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let mut status: libc::c_int = 0;
    let wait_res = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if wait_res == pid {
        // Child exited and was reaped
        return false;
    }
    if wait_res == 0 {
        // Child is still running
        return true;
    }
    // ECHILD (not our child) or other error: check kill(pid, 0)
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
pub fn is_pid_alive(_pid: i32) -> bool {
    false
}

pub struct DaemonRunner {
    config: DaemonConfig,
    retry_counts: HashMap<String, usize>,
    step_advancer: Option<StepAdvancer>,
    last_failure_output: HashMap<String, String>,
}

impl DaemonRunner {
    /// Creates a new `DaemonRunner` instance with the specified configuration.
    ///
    /// # Arguments
    /// * `config` - Daemon runtime configuration including state path, driver command, and retries.
    ///
    /// # Returns
    /// An initialized `DaemonRunner` with empty retry tracking and no custom step advancer.
    pub fn new(config: DaemonConfig) -> Result<Self, DaemonError> {
        Ok(Self {
            config,
            retry_counts: HashMap::new(),
            step_advancer: None,
            last_failure_output: HashMap::new(),
        })
    }

    /// Attaches a custom step advancer closure to decouple worktree merge and advancement.
    ///
    /// When attached, `run_step()` delegates state advancement and worktree transitions
    /// to this callback upon verification gate success rather than calling `advance_step()` in-process.
    ///
    /// # Arguments
    /// * `advancer` - A closure taking the state path and returning Ok(()) on success.
    pub fn with_step_advancer(mut self, advancer: StepAdvancer) -> Self {
        self.step_advancer = Some(advancer);
        self
    }

    /// Returns a reference to the unflattened verification failure outputs from previous retries.
    pub fn last_failure_output(&self) -> &HashMap<String, String> {
        &self.last_failure_output
    }

    /// Executes a single autonomous daemon step for the currently active task.
    ///
    /// # Process Lifecycle
    /// 1. Loads current execution state and determines the active task and working directory.
    /// 2. Renders a turnkey prompt, injecting previous verification failure diagnostics on retries.
    /// 3. Writes prompt to `.agent-state/prompts/<slug>.md` and exports `SUPERSTATE_PROMPT_FILE`.
    /// 4. Executes the headless driver worker, piping prompt to stdin. Non-zero exits log a warning
    ///    without aborting the step (driver rescue pattern).
    /// 5. Runs the task's verification command.
    /// 6. On gate pass: clears failure history, executes checkpoint commit if in-place,
    ///    and advances task via `step_advancer` hook or internal `advance_step`.
    /// 7. On gate failure: records diagnostics, stores failure output for retries, and transitions
    ///    to blocked if max retries exceeded.
    ///
    /// # Returns
    /// A `DaemonStepOutcome` indicating Advanced, Failed, or Blocked.
    ///
    /// # Errors
    /// Returns `DaemonError` if state loading, driver spawning, or advancer execution fails.
    pub fn run_step(&mut self) -> Result<DaemonStepOutcome, DaemonError> {
        let mut state = load_state(&self.config.state_path)?;

        // Ensure an active task is available to process
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

        // Determine working directory: prefer active worktree if present, else project root
        let working_dir = if let Some(ref wt) = state.active_worktree {
            wt.clone()
        } else {
            resolve_project_root(&self.config.state_path)
        };

        // Render turnkey prompt for the active task
        let mut prompt = render_turnkey_prompt(
            &state,
            &self.config.state_path,
            None,
            state.no_worktree,
            false,
        )
        .map_err(DaemonError::Driver)?;

        let verify_cmd = active_task.verification_cmd.as_deref().unwrap_or("true");

        // High-fidelity retry feedback: inject previous attempt diagnostics if retrying
        let retry_count = self.retry_counts.get(&active_task.id).copied().unwrap_or(0);
        if retry_count > 0 {
            if let Some(prev_err) = self.last_failure_output.get(&active_task.id) {
                let retry_section = format_retry_failure_section(
                    verify_cmd,
                    retry_count + 1,
                    self.config.max_retries,
                    prev_err,
                );
                prompt.push_str(&retry_section);
            }
        }

        // Multi-channel prompt delivery: write prompt to .agent-state/prompts/<slug>.md
        let (slug, base_dir) = resolve_daemon_slug_and_state_dir(&self.config.state_path);
        let prompts_dir = if base_dir.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
            base_dir.join("prompts")
        } else {
            base_dir.join(STATE_DIR).join("prompts")
        };
        fs::create_dir_all(&prompts_dir)?;
        let prompt_file = prompts_dir.join(format!("{}.md", slug));
        fs::write(&prompt_file, &prompt)?;

        // Execute headless driver worker if provided
        if let Some(ref driver) = self.config.driver_cmd {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(driver)
                .current_dir(&working_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("SUPERSTATE_PROMPT", &prompt)
                .env("SUPERSTATE_PROMPT_FILE", &prompt_file)
                .env("SUPERSTATE_ACTIVE_TASK", &active_task.id)
                .spawn()?;

            // Pipe prompt into driver stdin on a background thread to avoid pipe buffer deadlock
            // while the parent thread drains stdout/stderr in wait_with_output.
            if let Some(mut stdin) = child.stdin.take() {
                let prompt_bytes = prompt.as_bytes().to_vec();
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&prompt_bytes);
                });
            }

            let output = child.wait_with_output()?;

            // Driver rescue pattern: non-zero driver exit codes are logged as warnings
            // but do not abort daemon execution, allowing verification gate to determine outcome.
            // Captured stderr is bounded to 1,000 characters to prevent state file bloating.
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .chars()
                    .take(1000)
                    .collect::<String>();
                let code = output
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                state.notes.push(LedgerEntry::new(
                    "superstate-daemon",
                    format!(
                        "[driver-warn] Driver for task {} exited with code {}: {}",
                        active_task.id, code, stderr
                    ),
                ));
                save_state_atomic(&self.config.state_path, &state)?;
            }
        }

        // Execute verification gate to evaluate milestone completion
        let gate_res = run_verification_cmd(verify_cmd, &working_dir, self.config.timeout_secs)?;

        if gate_res.passed {
            self.retry_counts.remove(&active_task.id);
            self.last_failure_output.remove(&active_task.id);

            // Milestone git checkpoint in in-place mode when git repository exists
            if state.no_worktree && working_dir.join(".git").exists() {
                let status_res = Command::new("git")
                    .args(["status", "--porcelain"])
                    .current_dir(&working_dir)
                    .output();
                if let Ok(status_output) = status_res {
                    if !status_output.stdout.is_empty() {
                        let _ = Command::new("git")
                            .args(["add", "-A"])
                            .current_dir(&working_dir)
                            .output();
                        let commit_msg = format!(
                            "chore(superstate): checkpoint completed milestone {}: {}",
                            active_task.id, active_task.title
                        );
                        let _ = Command::new("git")
                            .args(["commit", "-m", &commit_msg])
                            .current_dir(&working_dir)
                            .output();
                    }
                }
            }

            // Step advancement: Pluggable StepAdvancer hook or fallback in-process advance
            if let Some(ref mut advancer) = self.step_advancer {
                advancer(&self.config.state_path).map_err(DaemonError::StepAdvancer)?;
                let updated_state = load_state(&self.config.state_path)?;
                let next_task_id = updated_state.active_task.as_ref().map(|t| t.id.clone());
                Ok(DaemonStepOutcome::Advanced {
                    completed_task_id: active_task.id,
                    next_task_id,
                })
            } else {
                advance_step(&mut state)?;
                save_state_atomic(&self.config.state_path, &state)?;

                let next_task_id = state.active_task.as_ref().map(|t| t.id.clone());
                Ok(DaemonStepOutcome::Advanced {
                    completed_task_id: active_task.id,
                    next_task_id,
                })
            }
        } else {
            // Verification gate failed: record failure diagnostics and update retry budget
            let retries = self.retry_counts.entry(active_task.id.clone()).or_insert(0);
            *retries += 1;

            let fail_reason = gate_res
                .failure_reason
                .clone()
                .unwrap_or_else(|| gate_res.output.clone());

            // Retain unflattened output for subsequent retry prompt generation
            let output_to_store = if !gate_res.output.trim().is_empty() {
                gate_res.output.clone()
            } else {
                fail_reason.clone()
            };
            self.last_failure_output
                .insert(active_task.id.clone(), output_to_store);

            state.notes.push(LedgerEntry::new(
                "superstate-daemon",
                format!(
                    "[verify-fail] Task {} failed: {}",
                    active_task.id, fail_reason
                ),
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
