use crate::impact::{build_sliced_test_cmd, infer_impacted_tests};
use crate::models::{ExecutionState, ExecutionStatus, LedgerEntry};
use crate::prompt::{format_retry_failure_section, render_turnkey_prompt, resolve_project_root};
use crate::state_machine::{
    advance_step, load_state, sanitize_branch_slug, save_state_atomic, StateError, STATE_DIR,
};
use crate::verifier::{run_verification_cmd, GateError};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;
use thiserror::Error;

/// Closure callback hook for pluggable step advancement across workspace and worktree lifecycles.
pub type StepAdvancer = Box<dyn FnMut(&Path) -> Result<(), String> + Send + 'static>;

/// Hard ceiling on attempts per task, as a multiple of `max_retries`.
///
/// `max_retries` counts *consecutive attempts that changed nothing*, so an agent that
/// keeps editing keeps earning attempts. This bounds that generosity so a worker that
/// churns the tree forever without ever passing its gate still terminates.
const PROGRESS_ATTEMPT_MULTIPLIER: usize = 10;

/// Directories never counted as progress.
///
/// `.agent-state` is written by the daemon itself on every attempt (prompts, logs, driver
/// turns), and test runners create `.pytest_cache` and `__pycache__` when executing gates,
/// so including them would make every attempt look like progress and the retry budget would
/// never expire.
const FINGERPRINT_SKIP_DIRS: [&str; 4] = [".git", ".agent-state", ".pytest_cache", "__pycache__"];

/// A verification command that cannot fail, so it certifies nothing.
///
/// The harness supplies one of these whenever a task's real verifier is sealed. Treating a
/// pass from it as evidence of completion makes the daemon stop after a single driver turn,
/// which is how it scored 50% against superstate_mcp's 70% on an identical task set while
/// running FASTER than the unassisted baseline -- it was not working harder, it was exiting
/// immediately on a gate that could not say no.
pub fn is_vacuous_gate(verify_cmd: &str) -> bool {
    matches!(
        verify_cmd.trim().trim_end_matches(';').trim(),
        "" | "true" | ":" | "exit 0" | "/bin/true"
    )
}

/// Check whether a path resides inside a git repository or worktree.
///
/// Supports subdirectories, standard repositories, and worktrees where `.git` may be
/// an ancestor directory or a gitlink file.
pub fn is_inside_git_repo(path: &Path) -> bool {
    let check_ancestors = |p: &Path| {
        p.ancestors()
            .any(|ancestor| !ancestor.as_os_str().is_empty() && ancestor.join(".git").exists())
    };

    if check_ancestors(path) {
        return true;
    }
    if let Ok(canonical) = path.canonicalize() {
        if check_ancestors(&canonical) {
            return true;
        }
    }
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(path)
        .output()
        .map(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Driver turns required before an UNGATED task may be reported complete.
///
/// Mirrors superstate_mcp's `max_turns_per_milestone`, whose iteration is turn-driven
/// rather than gate-driven and therefore degrades gracefully when the contract is vacuous.
/// Applies only when the gate cannot fail: a real contract still completes the moment it
/// passes, so this never prolongs a properly specified handoff.
const UNGATED_MIN_DRIVER_TURNS: usize = 3;

/// Upper bound on files walked, so a pathological tree cannot stall the daemon.
const FINGERPRINT_MAX_FILES: usize = 20_000;

/// Fingerprint of the working tree, used to distinguish "the worker changed something"
/// from "the worker spun without effect".
///
/// This walks the filesystem rather than asking git, because git cannot see the changes
/// that matter here. It collapses an untracked directory to a single `?? dir/` line no
/// matter what happens inside it, and `git diff HEAD` covers neither untracked files nor
/// a repository without a HEAD. A worker unpacking and editing a freshly cloned source
/// tree therefore produced a byte-identical git fingerprint on every attempt and was
/// blocked as stalled while it was in fact progressing.
fn workspace_fingerprint(dir: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    let mut entries: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&current) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(meta) = entry.metadata() else { continue };

            if meta.is_dir() {
                if !FINGERPRINT_SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else if entries.len() < FINGERPRINT_MAX_FILES {
                let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
                entries.push((rel, meta.len(), meta.modified().ok()));
            }
        }
    }

    // Directory iteration order is not stable across calls, so sort before hashing.
    entries.sort();
    for (path, size, modified) in &entries {
        path.hash(&mut hasher);
        size.hash(&mut hasher);
        if let Some(mtime) = modified {
            mtime.hash(&mut hasher);
        }
    }

    // Commits are progress even when they leave the tree byte-identical.
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    format!("{}|{}|{:x}", head, entries.len(), hasher.finish())
}

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
    #[error("Execution blocked for task {task_id}: {reason}")]
    Blocked { task_id: String, reason: String },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub state_path: PathBuf,
    pub driver_cmd: Option<String>,
    pub max_retries: usize,
    pub timeout_secs: u64,
    /// Treat a failing verification command as advisory once the retry budget is spent:
    /// advance anyway and leave the verdict to whatever grades the work, as
    /// `superstate_mcp` does. Default false -- completion should mean verified.
    #[serde(default)]
    pub advisory_gate: bool,
    /// Run advisory adversarial code review on milestone completion
    #[serde(default)]
    pub advisory_review: bool,
    /// Treat code review findings as blocking quality gates that fail milestone completion
    #[serde(default)]
    pub review_blocking: bool,
    /// Automatically roll back working tree to milestone baseline on verification failure
    #[serde(default = "default_true")]
    pub auto_rollback: bool,
    /// Automatically enable tool output shunting during driver worker execution
    #[serde(default = "default_true")]
    pub shunt_driver_output: bool,
    /// Automatically guard file reads and shunt large files to symbol outlines
    #[serde(default = "default_true")]
    pub guard_file_reads: bool,
    /// Enable milestone-scoped test impact slicing for fast fail-fast feedback
    #[serde(default)]
    pub impact_slicing: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            state_path: PathBuf::from(".agent-state/state.json"),
            driver_cmd: None,
            max_retries: 3,
            timeout_secs: 120,
            advisory_gate: false,
            advisory_review: false,
            review_blocking: false,
            auto_rollback: true,
            shunt_driver_output: true,
            guard_file_reads: true,
            impact_slicing: false,
        }
    }
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

impl DaemonStepOutcome {
    pub fn is_advanced(&self) -> bool {
        matches!(self, DaemonStepOutcome::Advanced { .. })
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, DaemonStepOutcome::Failed { .. })
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, DaemonStepOutcome::Blocked { .. })
    }
}

/// Structured completion artifact persisted upon task graph completion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionReport {
    pub status: ExecutionStatus,
    pub slug: String,
    pub base_branch: Option<String>,
    pub milestones_completed: usize,
    pub completed_at: String,
    pub worktree_merged: bool,
    #[serde(default)]
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
pub fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH"])
        .output();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        text.contains(&pid.to_string())
    } else {
        false
    }
}

/// Resolves a driver command string, expanding preset shorthands for known autonomous workers.
///
/// Headless drivers like Claude Code (`claude`) and Google Antigravity (`agy`) require non-interactive
/// autonomous flags (`-p` / `--print` and `--dangerously-skip-permissions` / `--permission-mode bypassPermissions`)
/// to avoid blocking on user confirmation prompts during unattended daemon execution.
///
/// # Arguments
/// * `driver` - Command string configured for the driver process.
///
/// # Returns
/// An expanded command string ensuring headless execution flags are present if targeting Claude or AGY,
/// or the original unmodified command string for custom driver workers.
pub fn resolve_driver_command(driver: &str) -> String {
    // Tokenize command line to inspect executable token and already-configured operational flags
    let tokens: Vec<&str> = driver.split_whitespace().collect();
    let first_token = match tokens.first() {
        Some(&t) => t,
        None => {
            // Guard against empty or whitespace-only driver commands by returning input untouched
            return driver.to_string();
        }
    };

    // Extract binary basename so custom binaries sharing a prefix (e.g. claude-code, agyd)
    // are not misidentified as Claude or AGY presets, while allowing path prefixes (e.g. /usr/bin/claude).
    let binary_name = std::path::Path::new(first_token)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(first_token);
    let is_claude = binary_name == "claude";
    let is_agy = binary_name == "agy";

    // Pass through custom or non-preset driver commands unmodified
    if !is_claude && !is_agy {
        return driver.to_string();
    }

    // Check for headless non-interactive print mode flag (-p or --print)
    let has_print = tokens.iter().any(|&t| t == "-p" || t == "--print");

    // Check for autonomous permission bypass flag:
    // Claude accepts --dangerously-skip-permissions or --permission-mode bypassPermissions.
    // AGY specifically requires --dangerously-skip-permissions.
    let has_perms = if is_claude {
        tokens.iter().any(|&t| {
            t == "--dangerously-skip-permissions"
                || t == "--permission-mode=bypassPermissions"
                || t.strip_prefix("--permission-mode=")
                    .map(|val| val.trim_matches('\'').trim_matches('"') == "bypassPermissions")
                    .unwrap_or(false)
        }) || tokens.windows(2).any(|w| {
            w[0] == "--permission-mode"
                && w[1].trim_matches('\'').trim_matches('"') == "bypassPermissions"
        })
    } else {
        // AGY driver preset requires --dangerously-skip-permissions specifically
        tokens.contains(&"--dangerously-skip-permissions")
    };

    // If both headless and permission bypass flags are satisfied, preserve command unmodified
    if has_print && has_perms {
        return driver.to_string();
    }

    // Trim trailing whitespace before appending missing autonomous flags to avoid double spaces
    let mut expanded = driver.trim_end().to_string();
    if !has_print {
        expanded.push_str(" -p");
    }
    if !has_perms {
        expanded.push_str(" --dangerously-skip-permissions");
    }

    expanded
}

pub struct DaemonRunner {
    config: DaemonConfig,
    /// Consecutive attempts that changed nothing. Reset whenever the worker makes progress.
    retry_counts: HashMap<String, usize>,
    /// Every attempt, progress or not, bounded by `PROGRESS_ATTEMPT_MULTIPLIER`.
    total_attempts: HashMap<String, usize>,
    /// Driver turns run per task, used to hold an ungated task open (see UNGATED_MIN_DRIVER_TURNS).
    driver_turns: HashMap<String, usize>,
    /// Working-tree fingerprint at the end of the previous attempt.
    last_fingerprint: HashMap<String, String>,
    /// Git HEAD commit hash when each milestone was first entered.
    pub milestone_start_commits: HashMap<String, String>,
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
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }

        Ok(Self {
            config,
            retry_counts: HashMap::new(),
            total_attempts: HashMap::new(),
            driver_turns: HashMap::new(),
            last_fingerprint: HashMap::new(),
            milestone_start_commits: HashMap::new(),
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

    /// Returns a reference to the recorded start commits for milestones.
    pub fn milestone_start_commits(&self) -> &HashMap<String, String> {
        &self.milestone_start_commits
    }

    /// Rolls back the working tree to the milestone start commit upon verification failure.
    fn perform_rollback(
        &mut self,
        task_id: &str,
        attempt: usize,
        working_dir: &Path,
        state: &mut ExecutionState,
    ) -> Result<(), DaemonError> {
        if !self.config.auto_rollback || !is_inside_git_repo(working_dir) {
            return Ok(());
        }

        let target_commit = match self.milestone_start_commits.get(task_id).cloned() {
            Some(c) => c,
            None => {
                // Resolve HEAD as fallback baseline if milestone_start_commits did not capture it
                if let Ok(out) = Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(working_dir)
                    .output()
                {
                    if out.status.success() {
                        let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if !h.is_empty() {
                            h
                        } else {
                            "HEAD".to_string()
                        }
                    } else {
                        "HEAD".to_string()
                    }
                } else {
                    "HEAD".to_string()
                }
            }
        };

        let _ = Command::new("git")
            .args(["reset", "--hard", &target_commit])
            .current_dir(working_dir)
            .output();
        let _ = Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(working_dir)
            .output();
        state.notes.push(LedgerEntry::new(
            "superstate-daemon",
            format!(
                "[rollback] Task {}: attempt {} failed verification; rolled back working tree to baseline commit {}. Avoid repeating previous failed approach.",
                task_id, attempt, target_commit
            ),
        ));
        save_state_atomic(&self.config.state_path, state)?;
        self.last_fingerprint
            .insert(task_id.to_string(), workspace_fingerprint(working_dir));
        Ok(())
    }

    /// Advances state under advisory_gate configuration when retries or attempts are exhausted.
    fn execute_advisory_advancement(
        &mut self,
        state: &mut ExecutionState,
        active_task_id: &str,
        note: &str,
    ) -> Result<DaemonStepOutcome, DaemonError> {
        state
            .notes
            .push(LedgerEntry::new("superstate-daemon", note));
        self.retry_counts.remove(active_task_id);
        self.last_failure_output.remove(active_task_id);
        self.milestone_start_commits.remove(active_task_id);

        if let Some(ref mut advancer) = self.step_advancer {
            save_state_atomic(&self.config.state_path, state)?;
            advancer(&self.config.state_path).map_err(DaemonError::StepAdvancer)?;
            let updated_state = load_state(&self.config.state_path)?;
            let next_task_id = updated_state.active_task.as_ref().map(|t| t.id.clone());
            if updated_state.status == ExecutionStatus::Completed {
                finalize_completed_execution(&self.config.state_path, &updated_state)?;
            }
            return Ok(DaemonStepOutcome::Advanced {
                completed_task_id: active_task_id.to_string(),
                next_task_id,
            });
        }
        match advance_step(state) {
            Ok(()) => {
                save_state_atomic(&self.config.state_path, state)?;
                let next_task_id = state.active_task.as_ref().map(|t| t.id.clone());
                if state.status == ExecutionStatus::Completed {
                    finalize_completed_execution(&self.config.state_path, state)?;
                }
                Ok(DaemonStepOutcome::Advanced {
                    completed_task_id: active_task_id.to_string(),
                    next_task_id,
                })
            }
            Err(StateError::InvalidTransition(msg)) => {
                state.status = ExecutionStatus::Blocked;
                state.notes.push(LedgerEntry::new(
                    "superstate-daemon",
                    format!(
                        "[dag-blocked] Milestone {} completed, but remaining tasks cannot advance: {}",
                        active_task_id, msg
                    ),
                ));
                save_state_atomic(&self.config.state_path, state)?;
                Ok(DaemonStepOutcome::Blocked {
                    task_id: active_task_id.to_string(),
                    reason: msg,
                })
            }
            Err(e) => Err(DaemonError::State(e)),
        }
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
    /// Executes a single autonomous daemon step for the currently active task.
    /// Alias for `run_step`.
    pub fn step(&mut self) -> Result<DaemonStepOutcome, DaemonError> {
        self.run_step()
    }

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

        // High-fidelity retry feedback: inject previous attempt diagnostics if retrying.
        //
        // Keyed on there being a previous failure, NOT on the stall counter: progress
        // resets that counter to zero, and gating on it would suppress the diagnostic in
        // exactly the case where the worker is converging and most needs the error back.
        let attempts_so_far = self
            .total_attempts
            .get(&active_task.id)
            .copied()
            .unwrap_or(0);
        {
            if let Some(prev_err) = self.last_failure_output.get(&active_task.id) {
                let retry_section = format_retry_failure_section(
                    verify_cmd,
                    attempts_so_far + 1,
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

        // Baseline the working tree before the worker touches it, so the first attempt's
        // changes count as progress rather than being charged as a stall.
        self.last_fingerprint
            .entry(active_task.id.clone())
            .or_insert_with(|| workspace_fingerprint(&working_dir));

        // Record starting HEAD commit when entering milestone so milestone diff captures all changes
        if !self.milestone_start_commits.contains_key(&active_task.id)
            && is_inside_git_repo(&working_dir)
        {
            if let Ok(out) = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&working_dir)
                .output()
            {
                if out.status.success() {
                    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !head.is_empty() {
                        self.milestone_start_commits
                            .insert(active_task.id.clone(), head);
                    }
                }
            }
        }

        // Execute headless driver worker if provided
        if let Some(ref driver) = self.config.driver_cmd {
            *self.driver_turns.entry(active_task.id.clone()).or_insert(0) += 1;
            let resolved_driver = resolve_driver_command(driver);
            let mut cmd = Command::new("sh");
            cmd.arg("-c")
                .arg(&resolved_driver)
                .current_dir(&working_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("SUPERSTATE_PROMPT", &prompt)
                .env("SUPERSTATE_PROMPT_FILE", &prompt_file)
                .env("SUPERSTATE_ACTIVE_TASK", &active_task.id)
                .env(
                    "SUPERSTATE_WORKTREE_PATH",
                    working_dir.to_string_lossy().as_ref(),
                );

            if self.config.shunt_driver_output {
                cmd.env("SUPERSTATE_SHUNT_ENABLED", "1");
                let bin_dir = working_dir.join(".agent-state").join("bin");
                fs::create_dir_all(&bin_dir)?;
                let shunt_wrapper = bin_dir.join("shunt");
                fs::write(
                    &shunt_wrapper,
                    "#!/bin/sh\nexec superstate shunt -- \"$@\"\n",
                )?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&shunt_wrapper, fs::Permissions::from_mode(0o755));
                }
                let current_paths = std::env::var_os("PATH").unwrap_or_default();
                let mut paths = vec![bin_dir];
                paths.extend(std::env::split_paths(&current_paths));
                if let Ok(new_path) = std::env::join_paths(paths) {
                    cmd.env("PATH", new_path);
                }
            }

            // WHY: Intercept driver file read operations (> 200 lines) transparently via PreToolUse hooks
            // WHAT: Provision .agents/hooks.json in the workspace root if not already present
            // HOW: Serialize PreToolUse matcher invoking `superstate guard pre-tool-use`
            if self.config.guard_file_reads {
                let agents_dir = working_dir.join(".agents");
                fs::create_dir_all(&agents_dir)?;
                let hooks_file = agents_dir.join("hooks.json");
                if !hooks_file.exists() {
                    let hooks_content = serde_json::json!({
                        "superstate-file-guard": {
                            "PreToolUse": [
                                {
                                    "matcher": "view_file|View|read_file",
                                    "hooks": [
                                        {
                                            "command": "superstate guard pre-tool-use --threshold-lines 200"
                                        }
                                    ]
                                }
                            ]
                        }
                    });
                    fs::write(&hooks_file, serde_json::to_string_pretty(&hooks_content)?)?;
                }
            }

            let mut child = cmd.spawn()?;

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

        // Stage 1: Milestone-Scoped Test Impact Slicing (Fast-Fail Pre-Filter)
        // Check impacted test suite before running full verification gate to ensure fast feedback.
        if self.config.impact_slicing && is_inside_git_repo(&working_dir) {
            let start_commit = self.milestone_start_commits.get(&active_task.id);
            let impacted = infer_impacted_tests(&working_dir, start_commit.map(|s| s.as_str()));
            if !impacted.is_empty() {
                if let Some(sliced_cmd) = build_sliced_test_cmd(verify_cmd, &impacted, &working_dir)
                {
                    let slice_start = Instant::now();
                    let slice_res =
                        run_verification_cmd(&sliced_cmd, &working_dir, self.config.timeout_secs)?;
                    let slice_elapsed = slice_start.elapsed().as_secs();

                    if !slice_res.passed {
                        // Extract diagnostic failure excerpt from slice output
                        let failure_output = if !slice_res.output.trim().is_empty() {
                            slice_res.output.trim()
                        } else if let Some(ref reason) = slice_res.failure_reason {
                            reason.trim()
                        } else {
                            "Unknown slice test failure"
                        };
                        let bounded_diag = failure_output.chars().take(1000).collect::<String>();

                        // Record fast-fail diagnostics into state notes
                        state.notes.push(LedgerEntry::new(
                            "superstate-daemon",
                            format!(
                                "[impact-fail] Task {}: Fast test slice failed in {}s: {}",
                                active_task.id, slice_elapsed, bounded_diag
                            ),
                        ));
                        save_state_atomic(&self.config.state_path, &state)?;

                        let fingerprint = workspace_fingerprint(&working_dir);
                        let made_progress = self
                            .last_fingerprint
                            .get(&active_task.id)
                            .is_some_and(|prev| prev != &fingerprint);
                        self.last_fingerprint
                            .insert(active_task.id.clone(), fingerprint);

                        // Update attempt counter and failure diagnostics for subsequent retry
                        let attempts = self
                            .total_attempts
                            .entry(active_task.id.clone())
                            .or_insert(0);
                        *attempts += 1;
                        let attempts_spent = *attempts;

                        let current_retries = {
                            let retries =
                                self.retry_counts.entry(active_task.id.clone()).or_insert(0);
                            if made_progress {
                                *retries = 0;
                                state.notes.push(LedgerEntry::new(
                                    "superstate-daemon",
                                    format!(
                                        "[retry-extend] Task {} changed the working tree on attempt {}; retry budget renewed",
                                        active_task.id, attempts_spent
                                    ),
                                ));
                            } else {
                                *retries += 1;
                            }
                            *retries
                        };

                        // Perform rollback to milestone baseline when auto_rollback is enabled
                        if self.config.auto_rollback && is_inside_git_repo(&working_dir) {
                            self.perform_rollback(
                                &active_task.id,
                                attempts_spent,
                                &working_dir,
                                &mut state,
                            )?;
                        }

                        self.last_failure_output
                            .insert(active_task.id.clone(), bounded_diag.clone());

                        let stalled = current_retries >= self.config.max_retries;
                        let exhausted =
                            attempts_spent >= self.config.max_retries * PROGRESS_ATTEMPT_MULTIPLIER;

                        if (stalled || exhausted) && self.config.advisory_gate {
                            let note = format!(
                                "[impact-advisory] Task {}: Fast slice failed but advisory_gate is enabled; proceeding to milestone completion",
                                active_task.id
                            );
                            return self.execute_advisory_advancement(
                                &mut state,
                                &active_task.id,
                                &note,
                            );
                        }

                        if stalled || exhausted {
                            state.status = ExecutionStatus::Blocked;
                            let reason = if stalled {
                                format!(
                                    "Task '{}' blocked: fast test slice failed: exceeded maximum retries ({}) without changing the working tree: {}",
                                    active_task.id, self.config.max_retries, bounded_diag
                                )
                            } else {
                                format!(
                                    "Task '{}' blocked: fast test slice failed: exceeded maximum attempts ({}): {}",
                                    active_task.id, attempts_spent, bounded_diag
                                )
                            };
                            state.errors.push(reason.clone());
                            save_state_atomic(&self.config.state_path, &state)?;
                            return Ok(DaemonStepOutcome::Blocked {
                                task_id: active_task.id.clone(),
                                reason,
                            });
                        }

                        save_state_atomic(&self.config.state_path, &state)?;
                        let retries_remaining =
                            self.config.max_retries.saturating_sub(current_retries);
                        return Ok(DaemonStepOutcome::Failed {
                            task_id: active_task.id,
                            reason: format!("Fast test slice failed: {}", bounded_diag),
                            retries_remaining,
                        });
                    } else {
                        // Record passing slice progress before executing full gate
                        state.notes.push(LedgerEntry::new(
                            "superstate-daemon",
                            format!(
                                "[impact-pass] Task {}: Fast slice passed ({} test files) in {}s, advancing to full gate",
                                active_task.id, impacted.len(), slice_elapsed
                            ),
                        ));
                        save_state_atomic(&self.config.state_path, &state)?;
                    }
                }
            }
        }

        // Execute verification gate to evaluate milestone completion
        let gate_res = run_verification_cmd(verify_cmd, &working_dir, self.config.timeout_secs)?;

        // A gate that cannot fail is not evidence of completion. Hold the task open and keep
        // driving until the turn floor is met, so an ungated handoff iterates like
        // superstate_mcp instead of exiting on the first unconditional pass.
        // Only meaningful when a driver is configured: with no worker there is nothing to
        // drive, the turn counter never advances, and holding the task open would loop
        // forever. Bounded by the attempt ceiling either way, so the floor can never
        // outlive the retry budget.
        let attempts_so_far = self
            .total_attempts
            .get(&active_task.id)
            .copied()
            .unwrap_or(0);
        let attempt_ceiling = self.config.max_retries * PROGRESS_ATTEMPT_MULTIPLIER;

        if gate_res.passed
            && (self.config.advisory_review || self.config.review_blocking)
            && is_inside_git_repo(&working_dir)
        {
            let start_commit = self
                .milestone_start_commits
                .get(&active_task.id)
                .map(|s| s.as_str());
            let diff = match superstate_review::diff::get_milestone_diff(&working_dir, start_commit)
            {
                Ok(d) if !d.trim().is_empty() => d,
                _ => superstate_review::get_git_diff(&working_dir, None).unwrap_or_default(),
            };
            let review_cfg = superstate_review::ReviewConfig {
                driver: self.config.driver_cmd.clone(),
                timeout_secs: self.config.timeout_secs,
            };
            if let Ok(report) = superstate_review::execute_review(
                &working_dir,
                &diff,
                Some(&active_task.title),
                &review_cfg,
            ) {
                if report.has_blocking_issues {
                    let mut blocking_findings: Vec<_> = report
                        .findings
                        .iter()
                        .filter(|f| {
                            matches!(
                                f.severity,
                                superstate_review::schema::Severity::Critical
                                    | superstate_review::schema::Severity::High
                            )
                        })
                        .take(3)
                        .collect();

                    if blocking_findings.is_empty() && !report.findings.is_empty() {
                        blocking_findings = report.findings.iter().take(3).collect();
                    }

                    let summary = report.summary.trim_end_matches('.');
                    let note = if blocking_findings.is_empty() {
                        format!("[review-advisory] Task {}: {}", active_task.id, summary)
                    } else {
                        let issues = blocking_findings
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                format!(
                                    "{}) {}: {} (fix: {})",
                                    i + 1,
                                    f.file.display(),
                                    f.problem,
                                    f.suggested_fix
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ");
                        format!(
                            "[review-advisory] Task {}: {}. Issues: {}",
                            active_task.id, summary, issues
                        )
                    };
                    state
                        .notes
                        .push(LedgerEntry::new("superstate-daemon", note));
                    save_state_atomic(&self.config.state_path, &state)?;

                    let should_block = report.has_blocking_issues
                        && (self.config.review_blocking || is_vacuous_gate(verify_cmd));

                    if should_block {
                        let blocking_count = report
                            .findings
                            .iter()
                            .filter(|f| {
                                matches!(
                                    f.severity,
                                    superstate_review::schema::Severity::Critical
                                        | superstate_review::schema::Severity::High
                                )
                            })
                            .count();
                        let count = if blocking_count > 0 {
                            blocking_count
                        } else {
                            report.findings.len()
                        };

                        if self.config.driver_cmd.is_some() && attempts_so_far < attempt_ceiling {
                            *self
                                .total_attempts
                                .entry(active_task.id.clone())
                                .or_insert(0) += 1;
                            let current_attempt = attempts_so_far + 1;
                            if self.config.auto_rollback && is_inside_git_repo(&working_dir) {
                                self.perform_rollback(
                                    &active_task.id,
                                    current_attempt,
                                    &working_dir,
                                    &mut state,
                                )?;
                            }
                            let reason = if self.config.review_blocking {
                                format!("code-review: {} blocking issue(s) detected", count)
                            } else {
                                "advisory-review: blocking issues detected on vacuous gate"
                                    .to_string()
                            };
                            return Ok(DaemonStepOutcome::Failed {
                                task_id: active_task.id,
                                reason,
                                retries_remaining: self
                                    .config
                                    .max_retries
                                    .saturating_sub(attempts_so_far),
                            });
                        } else if self.config.review_blocking && !self.config.advisory_gate {
                            state.status = ExecutionStatus::Blocked;
                            state.errors.push(format!(
                                "Task '{}' blocked: code review detected blocking issues and max retries ({}) exhausted",
                                active_task.id, self.config.max_retries
                            ));
                            save_state_atomic(&self.config.state_path, &state)?;
                            return Ok(DaemonStepOutcome::Blocked {
                                task_id: active_task.id,
                                reason:
                                    "code review detected blocking issues and max retries exhausted"
                                        .to_string(),
                            });
                        }
                    }
                }
            }
        }

        if gate_res.passed
            && is_vacuous_gate(verify_cmd)
            && self.config.driver_cmd.is_some()
            && attempts_so_far < attempt_ceiling
        {
            *self
                .total_attempts
                .entry(active_task.id.clone())
                .or_insert(0) += 1;
            let turns = self.driver_turns.entry(active_task.id.clone()).or_insert(0);
            if *turns < UNGATED_MIN_DRIVER_TURNS {
                let spent = *turns;
                state.notes.push(LedgerEntry::new(
                    "superstate-daemon",
                    format!(
                        "[ungated] Task {} has no falsifiable verification command; \
                         continuing to turn {} of {} rather than accepting an unconditional pass",
                        active_task.id,
                        spent + 1,
                        UNGATED_MIN_DRIVER_TURNS
                    ),
                ));
                save_state_atomic(&self.config.state_path, &state)?;
                return Ok(DaemonStepOutcome::Failed {
                    task_id: active_task.id,
                    reason: "ungated: turn floor not yet met".to_string(),
                    retries_remaining: UNGATED_MIN_DRIVER_TURNS - spent,
                });
            }
        }

        if gate_res.passed {
            self.retry_counts.remove(&active_task.id);
            self.last_failure_output.remove(&active_task.id);
            self.milestone_start_commits.remove(&active_task.id);

            // Milestone git checkpoint in in-place mode when git repository exists
            if state.no_worktree && is_inside_git_repo(&working_dir) {
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
                            .args(["commit", "--no-gpg-sign", "-m", &commit_msg])
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
                if updated_state.status == ExecutionStatus::Completed {
                    finalize_completed_execution(&self.config.state_path, &updated_state)?;
                } else if updated_state.status == ExecutionStatus::Blocked {
                    let reason = updated_state
                        .notes
                        .last()
                        .map(|n| n.note.clone())
                        .unwrap_or_else(|| "remaining tasks cannot advance".to_string());
                    return Ok(DaemonStepOutcome::Blocked {
                        task_id: active_task.id,
                        reason,
                    });
                }
                Ok(DaemonStepOutcome::Advanced {
                    completed_task_id: active_task.id,
                    next_task_id,
                })
            } else {
                // In-process advancement evaluates dependency satisfaction across pending milestones.
                // If remaining tasks have unresolved or cyclical dependencies, transition to Blocked
                // rather than failing unrecoverably, recording a ledger entry for epistemic tracking.
                match advance_step(&mut state) {
                    Ok(()) => {
                        save_state_atomic(&self.config.state_path, &state)?;

                        let next_task_id = state.active_task.as_ref().map(|t| t.id.clone());
                        if state.status == ExecutionStatus::Completed {
                            finalize_completed_execution(&self.config.state_path, &state)?;
                        }
                        Ok(DaemonStepOutcome::Advanced {
                            completed_task_id: active_task.id,
                            next_task_id,
                        })
                    }
                    Err(StateError::InvalidTransition(msg)) => {
                        // Guard against DAG deadlock: milestone completed successfully, but remaining
                        // pending tasks cannot advance due to missing or cyclical dependencies.
                        state.status = ExecutionStatus::Blocked;
                        state.notes.push(LedgerEntry::new(
                            "superstate-daemon",
                            format!(
                                "[dag-blocked] Milestone {} completed, but remaining tasks cannot advance: {}",
                                active_task.id, msg
                            ),
                        ));
                        save_state_atomic(&self.config.state_path, &state)?;
                        Ok(DaemonStepOutcome::Blocked {
                            task_id: active_task.id,
                            reason: msg,
                        })
                    }
                    Err(e) => Err(DaemonError::State(e)),
                }
            }
        } else {
            // Verification gate failed: record failure diagnostics and update retry budget.
            //
            // A fixed attempt cap gives up on an agent that is visibly converging while
            // wall-clock budget remains. `max_retries` therefore counts only consecutive
            // attempts that changed nothing: demonstrable progress buys a fresh budget,
            // the same way the surrounding harness grants lease extensions on new commits.
            // `PROGRESS_ATTEMPT_MULTIPLIER` bounds an agent that churns without converging.
            let fingerprint = workspace_fingerprint(&working_dir);
            let made_progress = self
                .last_fingerprint
                .get(&active_task.id)
                .is_some_and(|prev| prev != &fingerprint);
            self.last_fingerprint
                .insert(active_task.id.clone(), fingerprint);

            let attempts = self
                .total_attempts
                .entry(active_task.id.clone())
                .or_insert(0);
            *attempts += 1;
            let attempts_spent = *attempts;

            let current_retries = {
                let retries = self.retry_counts.entry(active_task.id.clone()).or_insert(0);
                if made_progress {
                    *retries = 0;
                    state.notes.push(LedgerEntry::new(
                        "superstate-daemon",
                        format!(
                            "[retry-extend] Task {} changed the working tree on attempt {}; retry budget renewed",
                            active_task.id, attempts_spent
                        ),
                    ));
                } else {
                    *retries += 1;
                }
                *retries
            };

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

            if self.config.auto_rollback && is_inside_git_repo(&working_dir) {
                self.perform_rollback(&active_task.id, attempts_spent, &working_dir, &mut state)?;
            }

            let stalled = current_retries >= self.config.max_retries;
            let exhausted = attempts_spent >= self.config.max_retries * PROGRESS_ATTEMPT_MULTIPLIER;
            if (stalled || exhausted) && self.config.advisory_gate {
                // Advisory: the verification command failed and the budget is spent, but the
                // work is submitted anyway, as superstate_mcp does. The sealed grader, not
                // the daemon, decides whether it was good enough.
                let note = format!(
                    "[advisory-gate] Task {} advancing with its verification command still \
                     failing after {} attempts; completion here does NOT mean verified",
                    active_task.id, attempts_spent
                );
                return self.execute_advisory_advancement(&mut state, &active_task.id, &note);
            }

            if stalled || exhausted {
                state.status = ExecutionStatus::Blocked;
                save_state_atomic(&self.config.state_path, &state)?;
                let reason = if stalled {
                    format!(
                        "Exceeded maximum retries ({}) without changing the working tree",
                        self.config.max_retries
                    )
                } else {
                    format!("Exceeded maximum attempts ({})", attempts_spent)
                };
                Ok(DaemonStepOutcome::Blocked {
                    task_id: active_task.id,
                    reason,
                })
            } else {
                save_state_atomic(&self.config.state_path, &state)?;
                Ok(DaemonStepOutcome::Failed {
                    task_id: active_task.id,
                    reason: fail_reason,
                    retries_remaining: self.config.max_retries - current_retries,
                })
            }
        }
    }

    /// Runs daemon milestone execution loop until all tasks reach terminal completion or failure.
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
                    return Err(DaemonError::Blocked { task_id, reason });
                }
            }
        }

        let final_state = load_state(&self.config.state_path)?;
        if final_state.status != ExecutionStatus::Completed {
            return Err(DaemonError::Driver(format!(
                "Cannot finalize execution: state status is {:?}, expected Completed",
                final_state.status
            )));
        }

        // Avoid redundant duplicate finalization if run_step already finalized and produced completion artifact
        let (slug, _) = resolve_daemon_slug_and_state_dir(&self.config.state_path);
        let parent = self
            .config
            .state_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let primary_completion_file = if slug == "default" {
            parent.join("completion.json")
        } else {
            parent.join(format!("completion-{}.json", slug))
        };

        if primary_completion_file.exists() {
            if let Ok(content) = fs::read_to_string(&primary_completion_file) {
                if let Ok(report) = serde_json::from_str::<CompletionReport>(&content) {
                    return Ok(report);
                }
            }
        }

        finalize_completed_execution(&self.config.state_path, &final_state)
    }
}

/// Finalizes execution when all milestones in the task graph reach completion.
///
/// If worktree isolation was active (`!state.no_worktree` and worktree was provisioned),
/// this automatically:
/// 1. Auto-commits any uncommitted changes inside the worktree checkout.
/// 2. Merges the worktree branch back into `base_branch` in the repository root.
/// 3. Cleans up the temporary worktree directory and branch.
/// 4. Updates state so `active_worktree = None` and `active_branch = None`.
/// 5. Atomically writes completion artifacts to `.agent-state/completion.json`
///    and `.agent-state/completions/<slug>.json`.
///
/// # Arguments
/// * `state_path` - Path to the state machine JSON file.
/// * `state` - The current execution state.
///
/// # Returns
/// A populated `CompletionReport` or `DaemonError`.
pub fn finalize_completed_execution(
    state_path: &Path,
    state: &ExecutionState,
) -> Result<CompletionReport, DaemonError> {
    // WHY: Auto-merge and completion artifact should only be finalized upon reaching terminal Completed status.
    // WHAT: Return an error if caller invokes finalization on uncompleted state.
    // HOW: Guard check status field against ExecutionStatus::Completed.
    if state.status != ExecutionStatus::Completed {
        return Err(DaemonError::Driver(format!(
            "Cannot finalize execution: state status is {:?}, expected Completed",
            state.status
        )));
    }

    let (slug, base_dir) = resolve_daemon_slug_and_state_dir(state_path);
    let state_dir = if base_dir.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
        base_dir
    } else {
        base_dir.join(STATE_DIR)
    };
    let repo_root = resolve_project_root(state_path);

    let mut worktree_merged = false;

    // WHY: In worktree isolation mode, the verified branch must be merged into base_branch before completion.
    // WHAT: Auto-commit uncommitted changes in worktree, checkout base_branch in repo_root, merge, and clean up.
    // HOW: Execute git commands via std::process::Command and clean up filesystem directories.
    if !state.no_worktree {
        if let Some(ref wt_path) = state.active_worktree {
            if wt_path.exists() {
                // Check if worktree checkout contains uncommitted changes
                let status_res = Command::new("git")
                    .args(["status", "--porcelain"])
                    .current_dir(wt_path)
                    .output();
                if let Ok(status_output) = status_res {
                    if !status_output.stdout.is_empty() {
                        let _ = Command::new("git")
                            .args(["add", "-A"])
                            .current_dir(wt_path)
                            .output();
                        let commit_msg =
                            format!("chore({}): auto-commit before daemon completion", slug);
                        let _ = Command::new("git")
                            .args(["commit", "-m", &commit_msg])
                            .current_dir(wt_path)
                            .output();
                    }
                }

                // Resolve branch name to merge
                let branch = state.active_branch.clone().unwrap_or_else(|| {
                    Command::new("git")
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(wt_path)
                        .output()
                        .ok()
                        .and_then(|o| {
                            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if s.is_empty() || s == "HEAD" {
                                None
                            } else {
                                Some(s)
                            }
                        })
                        .unwrap_or_else(|| format!("wt/{}", slug))
                });

                let base_branch = state
                    .base_branch
                    .clone()
                    .unwrap_or_else(|| "main".to_string());

                if is_inside_git_repo(&repo_root) {
                    // Ensure repository root is on base_branch before merging
                    let head_out = Command::new("git")
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(&repo_root)
                        .output();
                    if let Ok(out) = head_out {
                        let curr = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if curr != base_branch {
                            let co = Command::new("git")
                                .args(["checkout", &base_branch])
                                .current_dir(&repo_root)
                                .output();
                            if let Ok(co_out) = co {
                                if !co_out.status.success() {
                                    return Err(DaemonError::Driver(format!(
                                        "Failed to checkout base branch '{}': {}",
                                        base_branch,
                                        String::from_utf8_lossy(&co_out.stderr)
                                    )));
                                }
                            }
                        }
                    }

                    // Execute merge into base_branch
                    let merge_msg =
                        format!("chore(superstate): merge {} into {}", branch, base_branch);
                    let merge_res = Command::new("git")
                        .args(["merge", "--no-ff", "-m", &merge_msg, &branch])
                        .current_dir(&repo_root)
                        .output();

                    if let Ok(merge_out) = merge_res {
                        if !merge_out.status.success() {
                            let stderr = String::from_utf8_lossy(&merge_out.stderr).to_string();

                            // Handle conflicting untracked files in repo root
                            let status_output = Command::new("git")
                                .args(["status", "--porcelain"])
                                .current_dir(&repo_root)
                                .output()
                                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                                .unwrap_or_default();
                            let untracked_files: Vec<String> = status_output
                                .lines()
                                .filter(|l| l.starts_with("?? "))
                                .map(|l| l[3..].trim().to_string())
                                .collect();

                            let diff_cmd = format!("{}...{}", base_branch, branch);
                            let branch_files_output = Command::new("git")
                                .args(["diff", "--name-only", &diff_cmd])
                                .current_dir(&repo_root)
                                .output()
                                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                                .unwrap_or_default();
                            let branch_files: Vec<&str> =
                                branch_files_output.lines().map(|l| l.trim()).collect();

                            let mut cleaned_any = false;
                            for u in &untracked_files {
                                if branch_files.contains(&u.as_str()) {
                                    let target = repo_root.join(u);
                                    if target.is_file() {
                                        let _ = fs::remove_file(&target);
                                        cleaned_any = true;
                                    } else if target.is_dir() {
                                        let _ = fs::remove_dir_all(&target);
                                        cleaned_any = true;
                                    }
                                }
                            }

                            if cleaned_any {
                                let retry = Command::new("git")
                                    .args(["merge", "--no-ff", "-m", &merge_msg, &branch])
                                    .current_dir(&repo_root)
                                    .output();
                                if let Ok(retry_out) = retry {
                                    if !retry_out.status.success() {
                                        let _ = Command::new("git")
                                            .args(["merge", "--abort"])
                                            .current_dir(&repo_root)
                                            .output();
                                        return Err(DaemonError::Driver(format!(
                                            "Failed to merge worktree branch '{}': {}",
                                            branch,
                                            String::from_utf8_lossy(&retry_out.stderr)
                                        )));
                                    }
                                }
                            } else {
                                let _ = Command::new("git")
                                    .args(["merge", "--abort"])
                                    .current_dir(&repo_root)
                                    .output();
                                return Err(DaemonError::Driver(format!(
                                    "Failed to merge worktree branch '{}': {}",
                                    branch, stderr
                                )));
                            }
                        }
                    }

                    // Remove temporary worktree checkout via git
                    let _ = Command::new("git")
                        .args(["worktree", "remove", "--force", &wt_path.to_string_lossy()])
                        .current_dir(&repo_root)
                        .output();

                    // Remove directory if residual files remain
                    if wt_path.exists() {
                        let _ = fs::remove_dir_all(wt_path);
                    }

                    let _ = Command::new("git")
                        .args(["worktree", "prune"])
                        .current_dir(&repo_root)
                        .output();

                    // Delete feature branch if not identical to base_branch
                    if branch != base_branch {
                        let _ = Command::new("git")
                            .args(["branch", "-D", &branch])
                            .current_dir(&repo_root)
                            .output();
                    }

                    // Update state file on disk to reflect that worktree is cleaned up
                    if let Ok(mut current_state) = load_state(state_path) {
                        current_state.active_worktree = None;
                        current_state.active_branch = None;
                        let _ = save_state_atomic(state_path, &current_state);
                    }

                    worktree_merged = true;
                }
            } else {
                // Checkout directory already cleaned up
                worktree_merged = true;
            }
        } else {
            // Worktree was already merged and cleaned up (e.g. by step advancer)
            worktree_merged = true;
        }
    }

    // WHY: ADR 0002 mandates dual completion artifacts: single default and scoped per-job.
    // WHAT: Persist primary completion artifact (completion.json for default, completion-<slug>.json for branch)
    // and scoped completion in .agent-state/completions/<slug>.json.
    // HOW: Serialize CompletionReport to pretty JSON and write via temporary files atomically.
    let parent = state_path.parent().unwrap_or_else(|| Path::new("."));

    let primary_completion_file = if slug == "default" {
        parent.join("completion.json")
    } else {
        parent.join(format!("completion-{}.json", slug))
    };

    let completions_dir = state_dir.join("completions");
    let _ = fs::create_dir_all(&completions_dir);
    let slug_completion_file = completions_dir.join(format!("{}.json", slug));

    let report = CompletionReport {
        status: state.status.clone(),
        slug: slug.clone(),
        base_branch: state
            .base_branch
            .clone()
            .or_else(|| Some("main".to_string())),
        milestones_completed: state.completed_tasks.len(),
        completed_at: chrono::Utc::now().to_rfc3339(),
        worktree_merged,
        completion_file: primary_completion_file.clone(),
    };

    let serialized = serde_json::to_string_pretty(&report)?;

    // Atomically persist primary completion file
    let tmp_primary = primary_completion_file.with_extension("tmp");
    fs::write(&tmp_primary, &serialized)?;
    fs::rename(&tmp_primary, &primary_completion_file)?;

    // Atomically persist scoped completion file
    let tmp_slug = slug_completion_file.with_extension("tmp");
    fs::write(&tmp_slug, &serialized)?;
    fs::rename(&tmp_slug, &slug_completion_file)?;

    Ok(report)
}
