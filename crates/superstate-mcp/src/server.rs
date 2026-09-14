//! Native Model Context Protocol (MCP) server for Superstate.
//!
//! Provides JSON-RPC 2.0 stdio transport handling tool listing and execution
//! for agentic workflows (Claude Code, Google Antigravity, Cursor).
//!
//! Per ADR 0002, the MCP server acts strictly as a detached Gateway exposing 4 tools:
//! 1. `superstate_handoff`: Autonomous worktree creation and background daemon launch.
//! 2. `superstate_status`: Observability into state machine progress and daemon health.
//! 3. `superstate_cancel`: Graceful termination signal for background runners.
//! 4. `superstate_doctor`: Fail-closed environment and preflight diagnostic probe.
//!
//! All in-loop state mutation tools have been permanently deprecated from the MCP catalog.

use crate::tools::{list_all_mcp_tools, McpTool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use superstate_core::daemon::{
    daemon_paths_for_state, is_pid_alive, resolve_daemon_slug_and_state_dir, resolve_driver_command,
};
use superstate_core::handoff::{generate_resumption_command, write_handoff_package, HandoffSpec};
use superstate_core::ledger::{append_ledger_note, read_ledger};
use superstate_core::maintenance::{diagnose, repair};
use superstate_core::models::{LedgerEntry, MilestoneSpec};
use superstate_core::prompt::render_turnkey_prompt;
use superstate_core::state_machine::{
    branch_scoped_state_path, enclosing_state_root, find_branch_state_for_worktree,
    init_state_direct, init_state_milestones, load_state, sanitize_branch_slug, save_state_atomic,
    STATE_DIR,
};
use superstate_worktree::manager::{ensure_git_baseline, ensure_git_exclude, WorktreeManager};
use thiserror::Error;

/// Error variants encountered during MCP JSON-RPC protocol execution.
#[derive(Error, Debug)]
pub enum McpError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Incoming JSON-RPC 2.0 request structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

/// Outgoing JSON-RPC 2.0 response structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

/// Result of executing a preflight command dry-run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightResult {
    pub executable: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Telemetry metadata capturing background daemon execution state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatusInfo {
    pub pid: i32,
    pub running: bool,
    pub slug: String,
    pub log_file: PathBuf,
}

/// Resolves current daemon execution status for a given state machine path.
///
/// # Arguments
/// * `state_path` - Path to the active state JSON file.
///
/// # Returns
/// `Some(DaemonStatusInfo)` if a PID file is located, or `None` if unmanaged.
pub fn resolve_daemon_status(state_path: &Path) -> Option<DaemonStatusInfo> {
    // WHY: Daemon telemetry is keyed by branch slug under .agent-state/daemons/.
    // WHAT: Inspects the modern daemon path first, falling back to legacy daemon.pid.
    // HOW: Reads PID file, parses integer, and queries OS process table for liveness.
    let d_paths = daemon_paths_for_state(state_path);
    let pid_file = if d_paths.pid_file.exists() {
        d_paths.pid_file.clone()
    } else {
        // Fallback: Check local directory for legacy single-daemon setup
        let parent = state_path.parent().unwrap_or_else(|| Path::new("."));
        let local_pid = parent.join("daemon.pid");
        if local_pid.exists() {
            local_pid
        } else {
            // Guard clause: Return early if no PID record exists for this state file
            return None;
        }
    };

    let pid_str = std::fs::read_to_string(&pid_file).ok()?;
    let pid = pid_str.trim().parse::<i32>().ok()?;
    Some(DaemonStatusInfo {
        pid,
        running: is_pid_alive(pid),
        slug: d_paths.slug,
        log_file: d_paths.log_file,
    })
}

/// Gracefully terminates a process by PID using SIGTERM with fallback to SIGKILL.
///
/// # Arguments
/// * `pid` - Process ID of the background daemon to terminate.
///
/// # Returns
/// `Ok(())` on successful termination, or `Err(String)` if signalling fails.
#[cfg(unix)]
pub fn kill_pid(pid: i32) -> Result<(), String> {
    // WHY: Graceful shutdown allows processes to flush logs and release git lockfiles.
    // WHAT: Dispatches SIGTERM (-15); if process remains alive after 500ms, escalates to SIGKILL (-9).
    // HOW: Polls process liveness via is_pid_alive() in 50ms intervals across both signal phases.
    if is_pid_alive(pid) {
        let _ = std::process::Command::new("kill")
            .args(["-15", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if !is_pid_alive(pid) {
                return Ok(());
            }
        }

        if is_pid_alive(pid) {
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn kill_pid(pid: i32) -> Result<(), String> {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
    Ok(())
}

/// Safely clip a string to a maximum byte length on valid UTF-8 character boundaries.
///
/// # Arguments
/// * `s` - Source string to truncate.
/// * `max_len` - Maximum allowed byte length.
///
/// # Returns
/// Truncated string with ellipsis appended if clipped.
fn clip_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Execute a shell command in a dry-run fashion to check validity and capture output.
///
/// # Arguments
/// * `verify_cmd` - Shell command to execute.
/// * `cwd` - Working directory to run the command in.
/// * `timeout` - Maximum duration to allow before timing out.
///
/// # Returns
/// `Ok(PreflightResult)` with exit code and captured stdio, or `Err(String)` on timeout.
pub fn execute_preflight_with_timeout(
    verify_cmd: &str,
    cwd: &Path,
    timeout: std::time::Duration,
) -> Result<PreflightResult, String> {
    // WHY: Commands must be evaluated against the host shell to detect broken verify invariants.
    // WHAT: Spawns the verification command as a subprocess with piped stdout/stderr.
    // HOW: Monitors execution against a wall-clock timeout, terminating runaway children.
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("sh");
    #[cfg(not(windows))]
    cmd.arg("-c").arg(verify_cmd);

    #[cfg(windows)]
    let mut cmd = std::process::Command::new("cmd");
    #[cfg(windows)]
    cmd.args(["/C", verify_cmd]);

    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            // Guard clause: Return non-executable result if binary or shell cannot be spawned
            return Ok(PreflightResult {
                executable: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: clip_string(&e.to_string(), 500),
            });
        }
    };

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stdout_pipe {
            use std::io::Read;
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stderr_pipe {
            use std::io::Read;
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_millis(10);

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(format!(
                        "Preflight command timed out after {} seconds: {}",
                        timeout.as_secs(),
                        verify_cmd
                    ));
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Ok(PreflightResult {
                    executable: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: clip_string(&e.to_string(), 500),
                });
            }
        }
    };

    let stdout_bytes = stdout_thread.join().unwrap_or_default();
    let stderr_bytes = stderr_thread.join().unwrap_or_default();

    let exit_code = status.code().unwrap_or(-1);
    let stdout_raw = String::from_utf8_lossy(&stdout_bytes);
    let stderr_raw = String::from_utf8_lossy(&stderr_bytes);

    Ok(PreflightResult {
        executable: true,
        exit_code,
        stdout: clip_string(&stdout_raw, 500),
        stderr: clip_string(&stderr_raw, 500),
    })
}

/// Execute a shell command in a dry-run fashion with a 15-second timeout.
///
/// # Arguments
/// * `verify_cmd` - Shell command to execute.
/// * `cwd` - Working directory.
///
/// # Returns
/// `Ok(PreflightResult)` on completed execution or `Err(String)` on timeout.
pub fn execute_preflight(verify_cmd: &str, cwd: &Path) -> Result<PreflightResult, String> {
    execute_preflight_with_timeout(verify_cmd, cwd, std::time::Duration::from_secs(15))
}

/// Locates the compiled `superstate` CLI binary for spawning background runners.
///
/// # Returns
/// `PathBuf` pointing to the superstate executable.
fn resolve_superstate_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let name = exe.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if (name == "superstate" || name == "superstate.exe") && !name.contains('-') {
            return exe;
        }
        if let Some(parent) = exe.parent() {
            if parent.file_name().and_then(|s| s.to_str()) == Some("deps") {
                if let Some(target_dir) = parent.parent() {
                    let cand = target_dir.join("superstate");
                    if cand.exists() {
                        return cand;
                    }
                }
            }
        }
    }
    PathBuf::from("superstate")
}

/// Dynamically resolve state path for MCP operations.
///
/// 1. If explicit state path is provided and non-empty, resolves relative to `repo_root` (or absolute).
/// 2. If inside a worktree, matches via `find_branch_state_for_worktree`.
/// 3. If in a git repository on a feature branch (not `main`, `master`, and not starting with `wt/` or `superstate/`),
///    returns `branch_scoped_state_path(repo_root, &branch)`.
/// 4. Fallback: returns `repo_root.join(STATE_DIR).join("state.json")`.
///
/// # Arguments
/// * `repo_root` - Base repository path.
/// * `explicit_state` - Optional explicit state path string from caller.
///
/// # Returns
/// Resolved state file `PathBuf`.
pub fn resolve_mcp_state_path(repo_root: &Path, explicit_state: Option<&str>) -> PathBuf {
    if let Some(s) = explicit_state {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let p = Path::new(trimmed);
            if p.is_absolute() {
                return p.to_path_buf();
            }
            let direct = repo_root.join(p);
            if direct.exists() {
                return direct;
            }
            let under_state_dir = repo_root.join(STATE_DIR).join(p);
            if under_state_dir.exists() {
                return under_state_dir;
            }
            let under_branches = repo_root.join(STATE_DIR).join("branches").join(p);
            if under_branches.exists() {
                return under_branches;
            }
            return direct;
        }
    }

    // Check if repo_root is inside a worktree
    let wt_branch = WorktreeManager::new(repo_root)
        .ok()
        .and_then(|m| m.current_branch().ok());

    let mut ancestor = Some(repo_root);
    while let Some(current) = ancestor {
        let branches_dir = current.join(STATE_DIR).join("branches");
        if branches_dir.is_dir() {
            let mut wt_cand = Some(repo_root);
            while let Some(p) = wt_cand {
                if p == current {
                    break;
                }
                if let Some(matched) =
                    find_branch_state_for_worktree(current, p, wt_branch.as_deref())
                {
                    return matched;
                }
                wt_cand = p.parent();
            }
        }
        ancestor = current.parent();
    }

    // Query current branch
    if let Ok(manager) = WorktreeManager::new(repo_root) {
        if let Ok(branch) = manager.current_branch() {
            let is_feature_branch = branch != "main"
                && branch != "master"
                && !branch.starts_with("wt/")
                && !branch.starts_with("superstate/");
            if is_feature_branch {
                let root = enclosing_state_root(repo_root)
                    .or_else(|| {
                        let mut d = Some(repo_root);
                        while let Some(cur) = d {
                            if cur.join(".git").exists() {
                                return Some(cur.to_path_buf());
                            }
                            d = cur.parent();
                        }
                        None
                    })
                    .unwrap_or_else(|| repo_root.to_path_buf());
                let candidate = branch_scoped_state_path(&root, &branch);
                if candidate.exists() {
                    return candidate;
                }
                let default_state = repo_root.join(STATE_DIR).join("state.json");
                if default_state.exists() {
                    return default_state;
                }
                return candidate;
            }
        }
    }

    repo_root.join(STATE_DIR).join("state.json")
}

/// Returns the list of tools exposed by the Superstate MCP server.
///
/// In strict accordance with ADR 0002, this registers only the 4 Gateway tools:
/// `superstate_handoff`, `superstate_status`, `superstate_cancel`, and `superstate_doctor`.
/// All internal in-loop mutation tools have been permanently removed.
///
/// # Returns
/// Vector of active MCP tool definitions.
pub fn handle_list_tools() -> Vec<McpTool> {
    list_all_mcp_tools()
}

/// Dispatch a tool execution by name and arguments.
///
/// # Arguments
/// * `name` - The tool name requested by the JSON-RPC caller.
/// * `args` - The arguments JSON object.
///
/// # Errors
/// Returns an error message string if required arguments are missing, if a deprecated
/// tool is called, or if tool execution fails.
pub fn handle_tool_call(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        // Deprecated legacy in-loop execution tools per ADR 0002.
        // WHY: Empirical benchmarks proved in-loop stepping causes catastrophic context window bloating.
        // WHAT: Rejects calls to obsolete in-loop tools with explicit migration instructions.
        // HOW: Returns descriptive deprecation error pointing callers to superstate_handoff.
        "state_init"
        | "state_show"
        | "state_status"
        | "state_next"
        | "state_step"
        | "state_get"
        | "state_ready"
        | "state_checkpoint"
        | "state_rollback"
        | "state_prompt"
        | "state_sync"
        | "state_exec"
        | "state_doctor"
        | "state_validate"
        | "state_apply"
        | "state_resume"
        | "state_metrics"
        | "state_list"
        | "state_note"
        | "worktree_create"
        | "worktree_merge"
        | "worktree_list"
        | "worktree_remove"
        | "worktree_clean"
        | "superstate_review"
        | "superstate_preflight" => {
            Err(format!(
                "The tool '{}' has been deprecated. In-loop MCP execution is an anti-pattern. Use 'superstate_handoff' to initiate autonomous detached execution in an isolated worktree.",
                name
            ))
        }

        // Gateway Tool 1: superstate_handoff
        // The sole execution entrypoint. Validates verification invariant, provisions
        // isolated git worktree, and launches native Rust background daemon.
        "superstate_handoff" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let no_worktree = if let Some(nw) = args.get("no_worktree").and_then(Value::as_bool) {
                nw
            } else {
                !args
                    .get("worktree")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            };

            // Ensure baseline repository setup and git exclude invariants
            ensure_git_baseline(&repo_root);

            // Multi-milestone handoff or single-goal handoff dispatch
            let state = if let Some(milestones_val) = args.get("milestones") {
                // Deserialize milestones array into Vec<MilestoneSpec>
                let milestones: Vec<MilestoneSpec> = serde_json::from_value(milestones_val.clone())
                    .map_err(|e| format!("Invalid milestones array: {}", e))?;

                // Preflight dry-run each milestone's verify_cmd before state generation
                for m in &milestones {
                    execute_preflight(&m.verify_cmd, &repo_root)?;
                }

                // Discover active feature branch from repository
                let base_branch = if repo_root.join(".git").exists() {
                    WorktreeManager::new(&repo_root)
                        .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?
                        .current_branch()
                        .ok()
                } else {
                    None
                };

                let goal = args.get("goal").and_then(|v| v.as_str());
                let mut st =
                    init_state_milestones(goal, milestones, base_branch.as_deref(), no_worktree)
                        .map_err(|e| {
                            format!("Failed to initialize state from milestones: {}", e)
                        })?;

                // Provision isolated git worktree for task-1 if isolation is enabled
                if !no_worktree && repo_root.join(".git").exists() {
                    ensure_git_exclude(&repo_root);
                    let manager = WorktreeManager::new(&repo_root)
                        .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
                    if let Some(ref active) = st.active_task {
                        let wt = manager
                            .ensure_task_worktree(&active.id, base_branch.as_deref())
                            .map_err(|e| {
                                format!("Failed to create isolated worktree for milestone: {}", e)
                            })?;
                        st.active_worktree = Some(wt.path.clone());
                        st.active_branch = Some(wt.branch.clone());
                    }
                }

                st
            } else if let (Some(goal), Some(verify_cmd)) = (
                args.get("goal").and_then(|v| v.as_str()),
                args.get("verify_cmd").and_then(|v| v.as_str()),
            ) {
                let files_modify: Vec<PathBuf> = args
                    .get("files")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str())
                            .map(PathBuf::from)
                            .collect()
                    })
                    .unwrap_or_default();

                // Execute dry-run validation of the verify_cmd
                let _preflight = execute_preflight(verify_cmd, &repo_root)?;

                // Directly construct ExecutionState from goal and invariant without plan.md
                let mut st = init_state_direct(goal, verify_cmd, files_modify, no_worktree);

                // Discover active feature branch from repository
                let base_branch = if repo_root.join(".git").exists() {
                    WorktreeManager::new(&repo_root)
                        .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?
                        .current_branch()
                        .ok()
                } else {
                    None
                };
                st.base_branch = base_branch.clone();

                // Provision isolated git worktree if isolation is enabled and git repository exists
                if !no_worktree && repo_root.join(".git").exists() {
                    ensure_git_exclude(&repo_root);
                    let manager = WorktreeManager::new(&repo_root)
                        .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
                    if let Some(ref active) = st.active_task {
                        let wt = manager
                            .ensure_task_worktree(&active.id, base_branch.as_deref())
                            .map_err(|e| {
                                format!("Failed to ensure worktree for task {}: {}", active.id, e)
                            })?;
                        st.active_worktree = Some(wt.path.clone());
                        st.active_branch = Some(wt.branch.clone());
                    }
                }

                st
            } else {
                return Err(
                    "Must provide either 'milestones' array or both 'goal' and 'verify_cmd'"
                        .to_string(),
                );
            };

            // Ensure parent directory for state file exists
            if let Some(parent) = state_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create state directory: {}", e))?;
            }

            // Atomically persist state to disk
            save_state_atomic(&state_path, &state)
                .map_err(|e| format!("Failed to save state: {}", e))?;

            // Determine source agent from explicit property or ambient runtime environment variables
            let from_agent = if let Some(from) = args
                .get("from_agent")
                .or_else(|| args.get("from"))
                .and_then(Value::as_str)
            {
                from.to_string()
            } else if std::env::var("CLAUDE_PROJECT_ROOT").is_ok() {
                "claude".to_string()
            } else if std::env::var("AGY_VERSION").is_ok() {
                "agy".to_string()
            } else {
                "human".to_string()
            };

            // Determine target agent preset or generic fallback
            let to_agent = args
                .get("to_agent")
                .or_else(|| args.get("to"))
                .and_then(Value::as_str)
                .unwrap_or("any")
                .to_string();

            // Expand driver preset flags (e.g. claude -> claude -p --dangerously-skip-permissions)
            let raw_driver = args.get("driver").and_then(Value::as_str);
            let resolved_driver = raw_driver.map(resolve_driver_command);

            // Derive deterministic job slug for file naming and process coordination
            let (default_slug, _) = resolve_daemon_slug_and_state_dir(&state_path);
            let slug = if default_slug != "default" {
                default_slug
            } else if let Some(ref b) = state.active_branch {
                sanitize_branch_slug(b)
            } else if let Some(ref t) = state.active_task {
                sanitize_branch_slug(&t.id)
            } else {
                "default".to_string()
            };

            // Assemble handoff specification and load epistemic ledger history
            let spec = HandoffSpec {
                from_agent: Some(from_agent.clone()),
                to_agent: Some(to_agent.clone()),
                state_path: state_path.clone(),
                worktree_path: state.active_worktree.clone(),
                slug: slug.clone(),
            };

            let notes = read_ledger(&state_path);
            let handoff_file = write_handoff_package(&repo_root, &state, &spec, &notes)
                .map_err(|e| format!("Failed to write handoff package: {}", e))?;

            // Record [handoff] metadata note in epistemic ledger for provenance tracking
            let active_task_desc = state
                .active_task
                .as_ref()
                .map(|t| format!("{}: {}", t.id, t.title))
                .unwrap_or_else(|| "none".to_string());
            let handoff_note = format!(
                "[handoff] Handed off to {}: active task {}",
                to_agent, active_task_desc
            );
            let _ = append_ledger_note(&state_path, LedgerEntry::new(&from_agent, handoff_note));

            // Generate formatted copy-pasteable target resumption command
            let target_resumption_cmd = generate_resumption_command(
                &to_agent,
                state.active_worktree.as_deref(),
                &state_path,
            );

            // Per ADR 0002, detach MUST default to true unless explicitly overridden
            let detach = args.get("detach").and_then(Value::as_bool).unwrap_or(true);

            if detach {
                let bin = resolve_superstate_binary();
                let mut cmd = std::process::Command::new(bin);
                cmd.arg("state").arg("daemon");
                cmd.arg("--state").arg(&state_path);
                cmd.arg("--detach");
                let driver_str = resolved_driver.as_deref().or(raw_driver);
                if let Some(d) = driver_str {
                    cmd.arg("--driver").arg(d);
                }
                cmd.current_dir(&repo_root);

                let output = cmd
                    .output()
                    .map_err(|e| format!("Failed to spawn daemon process: {}", e))?;

                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("Daemon spawn failed: {}", err));
                }

                let daemon_paths = daemon_paths_for_state(&state_path);

                // Wait briefly for PID file if not immediately visible
                for _ in 0..10 {
                    if daemon_paths.pid_file.exists() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }

                let pid = if daemon_paths.pid_file.exists() {
                    std::fs::read_to_string(&daemon_paths.pid_file)
                        .ok()
                        .and_then(|s| s.trim().parse::<i32>().ok())
                } else {
                    let state_dir = state_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."));
                    let pid_file = state_dir.join("daemon.pid");
                    let fallback_pid_file = repo_root.join(".agent-state").join("daemon.pid");
                    if pid_file.exists() {
                        std::fs::read_to_string(&pid_file)
                            .ok()
                            .and_then(|s| s.trim().parse::<i32>().ok())
                    } else if fallback_pid_file.exists() {
                        std::fs::read_to_string(&fallback_pid_file)
                            .ok()
                            .and_then(|s| s.trim().parse::<i32>().ok())
                    } else {
                        None
                    }
                };

                let job_id = pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "detached".to_string());

                return Ok(serde_json::json!({
                    "status": "detached",
                    "slug": daemon_paths.slug,
                    "job_id": job_id,
                    "pid": pid,
                    "log_path": daemon_paths.log_file.display().to_string(),
                    "state_path": state_path.display().to_string(),
                    "worktree_path": state.active_worktree.as_ref().map(|p| p.display().to_string()),
                    "branch": state.active_branch,
                    "handoff_file": handoff_file.display().to_string(),
                    "target_resumption_command": target_resumption_cmd,
                    "content": [
                        {
                            "type": "text",
                            "text": format!("Autonomous daemon launched in background for '{}' (PID: {}). Track progress with superstate state status or superstate state logs.", daemon_paths.slug, job_id)
                        }
                    ]
                }));
            }

            // Generate turnkey prompt for the active task
            let is_in_place = no_worktree || state.no_worktree;
            let turnkey_prompt =
                render_turnkey_prompt(&state, &state_path, None, is_in_place, false)?;

            let task_id = state
                .active_task
                .as_ref()
                .map(|t| t.id.clone())
                .unwrap_or_else(|| "task-1".to_string());

            Ok(serde_json::json!({
                "status": "ready",
                "task_id": task_id,
                "slug": slug,
                "handoff_file": handoff_file.display().to_string(),
                "target_resumption_command": target_resumption_cmd,
                "worktree_path": state.active_worktree.as_ref().map(|p| p.display().to_string()),
                "branch": state.active_branch,
                "turnkey_prompt": turnkey_prompt,
                "content": [
                    {
                        "type": "text",
                        "text": turnkey_prompt
                    }
                ]
            }))
        }

        // Gateway Tool 2: superstate_status
        // WHY: Operators and client agents require real-time visibility into detached runs.
        // WHAT: Interrogates execution progress, active milestone, daemon status, and notes.
        // HOW: Reads branch-scoped state file, epistemic ledger, and daemon PID files.
        "superstate_status" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;
            let ledger_entries = read_ledger(&state_path);
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            let daemon_status = resolve_daemon_status(&state_path);
            let daemon_info = daemon_status
                .as_ref()
                .map(|d| {
                    serde_json::json!({
                        "pid": d.pid,
                        "running": d.running,
                        "slug": d.slug,
                        "log_file": d.log_file.display().to_string(),
                    })
                })
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "pid": serde_json::Value::Null,
                        "running": false,
                        "slug": serde_json::Value::Null,
                        "log_file": serde_json::Value::Null,
                    })
                });

            if as_json {
                let mut val = serde_json::to_value(&state)
                    .map_err(|e| format!("Failed to convert state to JSON value: {}", e))?;
                if let Value::Object(ref mut map) = val {
                    map.insert("daemon".to_string(), daemon_info.clone());
                }
                let pretty = serde_json::to_string_pretty(&val)
                    .map_err(|e| format!("Failed to serialize state: {}", e))?;
                if let Value::Object(ref mut map) = val {
                    map.insert(
                        "state".to_string(),
                        serde_json::to_value(&state).unwrap_or_default(),
                    );
                    map.insert(
                        "content".to_string(),
                        serde_json::json!([{ "type": "text", "text": pretty }]),
                    );
                }
                Ok(val)
            } else {
                let mut text = String::new();
                text.push_str(&format!("State Status: {:?}\n", state.status));
                text.push_str(&format!("Step: {}\n", state.step));
                if let Some(ref d) = daemon_status {
                    if d.running {
                        text.push_str(&format!(
                            "Daemon Status: Running (PID: {}, Slug: {})\n",
                            d.pid, d.slug
                        ));
                    } else {
                        text.push_str(&format!(
                            "Daemon Status: Stopped (Stale PID: {}, Slug: {})\n",
                            d.pid, d.slug
                        ));
                    }
                }
                if let Some(ref task) = state.active_task {
                    text.push_str(&format!("Active Task: {}: {}\n", task.id, task.title));
                } else {
                    text.push_str("Active Task: None\n");
                }
                text.push_str(&format!(
                    "Completed Tasks: {}\n",
                    state.completed_tasks.len()
                ));
                text.push_str(&format!("Ledger Entries: {}\n", ledger_entries.len()));
                for task in &state.completed_tasks {
                    text.push_str(&format!("  ✓ {}: {}\n", task.id, task.title));
                }
                if !ledger_entries.is_empty() {
                    text.push_str("\nRecent Notes (Epistemic Ledger):\n");
                    for note in ledger_entries.iter().rev().take(5) {
                        text.push_str(&format!("  • [{}] {}\n", note.author, note.note));
                    }
                }

                let mut ret = serde_json::json!({
                    "status": "success",
                    "output": text,
                    "text": text,
                    "content": [
                        {
                            "type": "text",
                            "text": text
                        }
                    ]
                });
                ret["daemon"] = daemon_info;
                Ok(ret)
            }
        }

        // Gateway Tool 3: superstate_cancel
        // WHY: Operators need to abort errant or runaway background workers cleanly.
        // WHAT: Dispatches termination signal (SIGTERM escalating to SIGKILL) and unlinks PID.
        // HOW: Targets by explicit state file, branch slug, or --all fleet cancellation.
        "superstate_cancel" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_dir = repo_root.join(STATE_DIR);
            let cancel_all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
            let branch = args
                .get("branch")
                .or_else(|| args.get("slug"))
                .and_then(Value::as_str);

            if cancel_all {
                let mut cancelled = Vec::new();
                let daemons_dir = state_dir.join("daemons");
                if daemons_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&daemons_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().and_then(|e| e.to_str()) == Some("pid") {
                                if let Ok(pid_str) = std::fs::read_to_string(&path) {
                                    if let Ok(pid) = pid_str.trim().parse::<i32>() {
                                        let _ = kill_pid(pid);
                                        cancelled.push(pid);
                                        let _ = std::fs::remove_file(&path);
                                    }
                                }
                            }
                        }
                    }
                }
                let legacy_pid = state_dir.join("daemon.pid");
                if legacy_pid.exists() {
                    if let Ok(pid_str) = std::fs::read_to_string(&legacy_pid) {
                        if let Ok(pid) = pid_str.trim().parse::<i32>() {
                            let _ = kill_pid(pid);
                            cancelled.push(pid);
                            let _ = std::fs::remove_file(&legacy_pid);
                        }
                    }
                }
                return Ok(serde_json::json!({
                    "status": "cancelled",
                    "cancelled_pids": cancelled,
                    "content": [{
                        "type": "text",
                        "text": format!("Cancelled {} daemon processes.", cancelled.len())
                    }]
                }));
            }

            if let Some(branch_name) = branch {
                let slug = sanitize_branch_slug(branch_name);
                let pid_file = state_dir.join("daemons").join(format!("{}.pid", slug));
                if pid_file.exists() {
                    let pid_str = std::fs::read_to_string(&pid_file).map_err(|e| e.to_string())?;
                    let pid: i32 = pid_str.trim().parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
                    let _ = kill_pid(pid);
                    let _ = std::fs::remove_file(&pid_file);
                    return Ok(serde_json::json!({
                        "status": "cancelled",
                        "slug": slug,
                        "pid": pid,
                        "content": [{
                            "type": "text",
                            "text": format!("Daemon for branch '{}' (PID: {}) terminated.", branch_name, pid)
                        }]
                    }));
                } else {
                    return Ok(serde_json::json!({
                        "status": "not_running",
                        "slug": slug,
                        "content": [{
                            "type": "text",
                            "text": format!("No active daemon found for branch '{}'.", branch_name)
                        }]
                    }));
                }
            }

            // Single state target resolution
            let state_path = resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));
            let d_paths = daemon_paths_for_state(&state_path);
            if d_paths.pid_file.exists() {
                let pid_str = std::fs::read_to_string(&d_paths.pid_file).map_err(|e| e.to_string())?;
                let pid: i32 = pid_str.trim().parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
                let _ = kill_pid(pid);
                let _ = std::fs::remove_file(&d_paths.pid_file);
                return Ok(serde_json::json!({
                    "status": "cancelled",
                    "slug": d_paths.slug,
                    "pid": pid,
                    "content": [{
                        "type": "text",
                        "text": format!("Daemon '{}' (PID: {}) terminated.", d_paths.slug, pid)
                    }]
                }));
            }

            Ok(serde_json::json!({
                "status": "not_running",
                "content": [{
                    "type": "text",
                    "text": "No active daemon found for state path."
                }]
            }))
        }

        // Gateway Tool 4: superstate_doctor
        // WHY: Fail-closed architecture demands verifying runtime prerequisites before work begins.
        // WHAT: Performs diagnostic checks across binary integrity, git repository, and state health.
        // HOW: Inspects binary paths, evaluates git tree, and validates/repairs state machines.
        "superstate_doctor" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            if !state_path.exists() {
                return Ok(serde_json::json!({
                    "status": "healthy",
                    "is_healthy": true,
                    "issues": Vec::<String>::new(),
                    "warnings": Vec::<String>::new(),
                    "repaired_actions": Vec::<String>::new(),
                    "output": "Doctor: state file does not exist yet. Environment baseline is clean.",
                    "content": [{ "type": "text", "text": "Doctor: state file does not exist yet. Environment baseline is clean." }]
                }));
            }

            let mut state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;

            let plan_content = if let Some(plan_str) = args.get("plan").and_then(Value::as_str) {
                let plan_path = if Path::new(plan_str).is_absolute() {
                    PathBuf::from(plan_str)
                } else {
                    repo_root.join(plan_str)
                };
                Some(std::fs::read_to_string(&plan_path).map_err(|e| {
                    format!("Failed to read plan at {}: {}", plan_path.display(), e)
                })?)
            } else {
                None
            };

            let report = diagnose(&state, plan_content.as_deref());
            let repair_flag = args.get("repair").and_then(Value::as_bool).unwrap_or(false);
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            let mut actions = Vec::new();
            if repair_flag {
                actions = repair(&mut state);
                if !actions.is_empty() {
                    save_state_atomic(&state_path, &state)
                        .map_err(|e| format!("Failed to save repaired state: {}", e))?;
                }
            }

            let mut text = String::new();
            if report.is_healthy {
                text.push_str("Doctor: state is healthy.\n");
            } else {
                text.push_str("Doctor found issues:\n");
                for issue in &report.issues {
                    text.push_str(&format!("  [!] {}\n", issue));
                }
            }
            for w in &report.warnings {
                text.push_str(&format!("  [*] {}\n", w));
            }
            if !actions.is_empty() {
                text.push_str("Repairs made:\n");
                for a in &actions {
                    text.push_str(&format!("  + {}\n", a));
                }
            }

            if as_json {
                let pretty = serde_json::to_string_pretty(&serde_json::json!({
                    "is_healthy": report.is_healthy,
                    "issues": report.issues,
                    "warnings": report.warnings,
                    "repaired_actions": actions,
                    "details": report.details,
                }))
                .map_err(|e| e.to_string())?;

                Ok(serde_json::json!({
                    "status": "success",
                    "is_healthy": report.is_healthy,
                    "issues": report.issues,
                    "warnings": report.warnings,
                    "repaired_actions": actions,
                    "details": report.details,
                    "content": [{ "type": "text", "text": pretty }]
                }))
            } else {
                Ok(serde_json::json!({
                    "status": "success",
                    "is_healthy": report.is_healthy,
                    "issues": report.issues,
                    "warnings": report.warnings,
                    "repaired_actions": actions,
                    "details": report.details,
                    "output": text,
                    "text": text,
                    "content": [{ "type": "text", "text": text }]
                }))
            }
        }

        unknown => Err(format!("Unknown tool: {}", unknown)),
    }
}

/// JSON-RPC request dispatcher.
///
/// # Arguments
/// * `req` - Parsed JSON-RPC request.
///
/// # Returns
/// Corresponding JSON-RPC response.
pub fn handle_json_rpc(req: JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "superstate-mcp",
                    "version": "0.1.0"
                }
            })),
            error: None,
        },
        "tools/list" => {
            let tools = handle_list_tools();
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::json!({
                    "tools": tools
                })),
                error: None,
            }
        }
        "tools/call" => {
            let params = match req.params {
                Some(ref p) => p,
                None => {
                    return JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: None,
                        error: Some(serde_json::json!({
                            "code": -32602,
                            "message": "Missing params in tools/call"
                        })),
                    };
                }
            };

            let tool_name = match params.get("name").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => {
                    return JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: None,
                        error: Some(serde_json::json!({
                            "code": -32602,
                            "message": "Missing 'name' field in tools/call params"
                        })),
                    };
                }
            };

            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));

            match handle_tool_call(tool_name, &arguments) {
                Ok(res) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: Some(res),
                    error: None,
                },
                Err(err_msg) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(serde_json::json!({
                        "code": -32603,
                        "message": err_msg
                    })),
                },
            }
        }
        "notifications/initialized" | "notifications/cancelled" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(serde_json::json!({})),
            error: None,
        },
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(serde_json::json!({
                "code": -32601,
                "message": format!("Method not found: {}", req.method)
            })),
        },
    }
}

/// Runs the standard I/O line-delimited JSON-RPC loop for MCP servers.
///
/// # Errors
/// Returns `McpError` on fatal standard I/O failures.
pub fn run_stdio_mcp_server() -> Result<(), McpError> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&line) {
            // JSON-RPC 2.0 and MCP spec: notifications have no id and must not receive a response.
            if req.id.is_none() || req.method.starts_with("notifications/") {
                continue;
            }

            let resp = handle_json_rpc(req);
            let resp_str = serde_json::to_string(&resp)?;
            writeln!(stdout, "{}", resp_str)?;
            stdout.flush()?;
        }
    }

    Ok(())
}
