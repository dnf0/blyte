//! Native Model Context Protocol (MCP) server for Superstate.
//!
//! Provides JSON-RPC 2.0 stdio transport handling tool listing and execution
//! for agentic workflows (Claude Code, Antigravity, Cursor).

use crate::tools::list_all_mcp_tools;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use superstate_core::daemon::{daemon_paths_for_state, is_pid_alive};
use superstate_core::impact::infer_impacted_tests;
use superstate_core::ledger::{append_ledger_note, read_ledger};
use superstate_core::maintenance::{diagnose, metrics, repair, resume_briefing, sync_with_plan};
use superstate_core::models::{ExecutionStatus, LedgerEntry, MilestoneSpec};
use superstate_core::patching::{apply_patch, validate_state};
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::prompt::render_turnkey_prompt;
use superstate_core::resolve_project_root;
use superstate_core::state_machine::{
    advance_step, branch_scoped_state_path, find_branch_state_for_worktree, init_state_direct,
    init_state_milestones, load_state, ready_tasks, save_state_atomic, STATE_DIR,
};
use superstate_core::verifier::execute_all_gates_filtered;
use superstate_worktree::manager::{
    ensure_git_baseline, ensure_git_exclude, IsolatedWorktree, WorktreeManager,
};
use superstate_worktree::types::WorktreeConfig;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum McpError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightResult {
    pub executable: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatusInfo {
    pub pid: i32,
    pub running: bool,
    pub slug: String,
    pub log_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchStateSummary {
    pub branch: String,
    pub status: ExecutionStatus,
    pub active_task: Option<String>,
    pub step: usize,
    pub total_steps: usize,
    pub base_branch: Option<String>,
    pub state_file: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonStatusInfo>,
}

/// Resolves current daemon execution status for a given state machine path.
pub fn resolve_daemon_status(state_path: &Path) -> Option<DaemonStatusInfo> {
    let d_paths = daemon_paths_for_state(state_path);
    let pid_file = if d_paths.pid_file.exists() {
        d_paths.pid_file.clone()
    } else {
        let parent = state_path.parent().unwrap_or_else(|| Path::new("."));
        let local_pid = parent.join("daemon.pid");
        if local_pid.exists() {
            local_pid
        } else {
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

fn save_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory {}: {}", parent.display(), e))?;
        }
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp_file_path = parent.join(format!(".state.{}.tmp", std::process::id()));
    let serialized = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    std::fs::write(&temp_file_path, serialized)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    std::fs::rename(&temp_file_path, path)
        .map_err(|e| format!("Failed to atomically save {}: {}", path.display(), e))?;
    Ok(())
}

/// Safely clip a string to a maximum byte length on valid UTF-8 character boundaries.
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
pub fn execute_preflight_with_timeout(
    verify_cmd: &str,
    cwd: &Path,
    timeout: std::time::Duration,
) -> Result<PreflightResult, String> {
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
pub fn execute_preflight(verify_cmd: &str, cwd: &Path) -> Result<PreflightResult, String> {
    execute_preflight_with_timeout(verify_cmd, cwd, std::time::Duration::from_secs(15))
}

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
                let root = superstate_core::state_machine::enclosing_state_root(repo_root)
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
                return branch_scoped_state_path(&root, &branch);
            }
        }
    }

    repo_root.join(STATE_DIR).join("state.json")
}

/// Dispatch a tool execution by name and arguments.
///
/// # Arguments
/// * `name` - The tool name requested by the JSON-RPC caller.
/// * `args` - The arguments JSON object.
///
/// # Errors
/// Returns an error message string if required arguments are missing or tool execution fails.
pub fn handle_tool_call(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "superstate_preflight" => {
            let cwd = args
                .get("cwd")
                .or_else(|| args.get("repo_root"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let _state_path =
                resolve_mcp_state_path(&cwd, args.get("state").and_then(Value::as_str));

            // If verify_cmds array is provided, iterate and dry-run each command
            if let Some(cmds_val) = args.get("verify_cmds") {
                let cmds: Vec<String> = cmds_val
                    .as_array()
                    .ok_or_else(|| "'verify_cmds' must be an array of strings".to_string())?
                    .iter()
                    .map(|v| {
                        v.as_str().map(ToString::to_string).ok_or_else(|| {
                            "Each entry in 'verify_cmds' must be a string".to_string()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let mut results = Vec::new();
                let mut all_executable = true;
                let mut all_passed = true;
                let mut aggregate_stdout = String::new();
                let mut aggregate_stderr = String::new();

                // Validate each verification command independently and aggregate outcomes
                for cmd in &cmds {
                    let res = execute_preflight(cmd, &cwd)?;
                    if !res.executable {
                        all_executable = false;
                        all_passed = false;
                    }
                    if res.exit_code != 0 {
                        all_passed = false;
                    }
                    if !res.stdout.is_empty() {
                        if !aggregate_stdout.is_empty() {
                            aggregate_stdout.push('\n');
                        }
                        aggregate_stdout.push_str(&res.stdout);
                    }
                    if !res.stderr.is_empty() {
                        if !aggregate_stderr.is_empty() {
                            aggregate_stderr.push('\n');
                        }
                        aggregate_stderr.push_str(&res.stderr);
                    }
                    results.push(serde_json::json!({
                        "command": cmd,
                        "executable": res.executable,
                        "exit_code": res.exit_code,
                        "stdout": res.stdout,
                        "stderr": res.stderr,
                    }));
                }

                return Ok(serde_json::json!({
                    "executable": all_executable,
                    "exit_code": if all_passed { 0 } else { 1 },
                    "all_passed": all_passed,
                    "results": results,
                    "stdout": aggregate_stdout,
                    "stderr": aggregate_stderr,
                    "content": [
                        {
                            "type": "text",
                            "text": format!("preflight executed {} commands, all_passed: {}", results.len(), all_passed)
                        }
                    ]
                }));
            }

            let verify_cmd = args
                .get("verify_cmd")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'verify_cmd'".to_string())?;

            let res = execute_preflight(verify_cmd, &cwd)?;

            Ok(serde_json::json!({
                "executable": res.executable,
                "exit_code": res.exit_code,
                "stdout": res.stdout,
                "stderr": res.stderr,
                "content": [
                    {
                        "type": "text",
                        "text": format!("executable: {}, exit_code: {}", res.executable, res.exit_code)
                    }
                ]
            }))
        }
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
                                format!("Failed to ensure worktree for task {}: {}", active.id, e)
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

            // Ensure parent directory exists for branch-scoped state
            if let Some(parent) = state_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create state directory: {}", e))?;
            }

            // Atomically persist state to disk
            save_state_atomic(&state_path, &state)
                .map_err(|e| format!("Failed to save state: {}", e))?;

            let detach = args.get("detach").and_then(Value::as_bool).unwrap_or(false);
            let driver = args.get("driver").and_then(Value::as_str);

            if detach {
                let bin = resolve_superstate_binary();
                let mut cmd = std::process::Command::new(bin);
                cmd.arg("state").arg("daemon");
                cmd.arg("--state").arg(&state_path);
                cmd.arg("--detach");
                if let Some(d) = driver {
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
        "state_init" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            if let Some(parent) = state_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create state directory: {}", e))?;
            }

            ensure_git_baseline(&repo_root);

            let no_worktree = if let Some(nw) = args.get("no_worktree").and_then(Value::as_bool) {
                nw
            } else {
                !args
                    .get("worktree")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            };

            let base_branch = if repo_root.join(".git").exists() {
                WorktreeManager::new(&repo_root)
                    .ok()
                    .and_then(|m| m.current_branch().ok())
            } else {
                None
            };

            let mut state = if let Some(plan) = args.get("plan").and_then(|v| v.as_str()) {
                let plan_path = PathBuf::from(plan);
                let mut st = init_state_from_plan(&plan_path, &state_path)
                    .map_err(|e| format!("Plan parse error: {}", e))?;
                if st.base_branch.is_none() {
                    st.base_branch = base_branch.clone();
                }
                st.no_worktree = no_worktree;
                st
            } else if let Some(milestones_val) = args.get("milestones") {
                let milestones: Vec<MilestoneSpec> = serde_json::from_value(milestones_val.clone())
                    .map_err(|e| format!("Invalid milestones array: {}", e))?;
                let goal = args.get("goal").and_then(|v| v.as_str());
                init_state_milestones(goal, milestones, base_branch.as_deref(), no_worktree)
                    .map_err(|e| format!("Failed to initialize state from milestones: {}", e))?
            } else if let Some(goal) = args.get("goal").and_then(|v| v.as_str()) {
                let verify_cmd = args
                    .get("verify_cmd")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing required argument 'verify_cmd'".to_string())?;
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
                let mut st = init_state_direct(goal, verify_cmd, files_modify, no_worktree);
                st.base_branch = base_branch.clone();
                st
            } else {
                return Err(
                    "Must provide either 'plan', 'goal' with 'verify_cmd', or 'milestones'"
                        .to_string(),
                );
            };

            if !no_worktree && repo_root.join(".git").exists() {
                ensure_git_exclude(&repo_root);
                let manager = WorktreeManager::new(&repo_root)
                    .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
                if let Some(ref active) = state.active_task {
                    let wt = manager
                        .ensure_task_worktree(&active.id, base_branch.as_deref())
                        .map_err(|e| {
                            format!("Failed to ensure worktree for task {}: {}", active.id, e)
                        })?;
                    state.active_worktree = Some(wt.path.clone());
                    state.active_branch = Some(wt.branch.clone());
                }
            }

            save_state_atomic(&state_path, &state)
                .map_err(|e| format!("Failed to save state: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "step": state.step,
                "active_task": state.active_task
            }))
        }
        "state_next" | "state_step" => {
            let verify_stage = args
                .get("verify_stage")
                .and_then(|v| v.as_str())
                .unwrap_or("milestone");

            if verify_stage != "milestone" && verify_stage != "final" {
                return Err(format!(
                    "Invalid verify_stage '{}'. Must be 'milestone' or 'final'",
                    verify_stage
                ));
            }

            let verify_cmd = args
                .get("verify_cmd")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(ToString::to_string);

            let verify_git_clean = args
                .get("verify_git_clean")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let ignore_untracked_glob = args
                .get("ignore_untracked_glob")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let mut state =
                load_state(&state_path).map_err(|e| format!("Load state error: {}", e))?;

            let active_task = match state.active_task.clone() {
                Some(t) => t,
                None => return Err("No active task to advance".to_string()),
            };

            let mut task_to_verify = active_task.clone();
            if let Some(cmd) = verify_cmd {
                task_to_verify.verification_cmd = Some(cmd);
            }

            ensure_git_exclude(&repo_root);

            let target_dir = state
                .active_worktree
                .as_ref()
                .filter(|p| p.exists())
                .cloned()
                .unwrap_or_else(|| repo_root.clone());

            // Progressive staged verification: infer impacted tests for milestone stage when verification_cmd is empty
            let mut impacted_tests_list: Vec<String> = Vec::new();
            let mut synthesized_verify_cmd_opt: Option<String> = None;

            if verify_stage == "milestone"
                && task_to_verify
                    .verification_cmd
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                // Discover impacted tests against base branch in target directory or repository root
                let check_dir = if target_dir.exists() {
                    &target_dir
                } else {
                    &repo_root
                };

                let mut impacted = infer_impacted_tests(check_dir, state.base_branch.as_deref());
                // Fall back to repo_root discovery if target_dir returned no impacted tests
                if impacted.is_empty() && check_dir != &repo_root {
                    impacted = infer_impacted_tests(&repo_root, None);
                }

                if !impacted.is_empty() {
                    // Extract relative path strings for response payload
                    let rel_tests: Vec<String> = impacted
                        .iter()
                        .map(|p| {
                            p.strip_prefix(check_dir)
                                .or_else(|_| p.strip_prefix(&repo_root))
                                .unwrap_or(p)
                                .display()
                                .to_string()
                        })
                        .collect();

                    synthesized_verify_cmd_opt =
                        synthesize_targeted_test_command(check_dir, &impacted);

                    impacted_tests_list = rel_tests;

                    // Enforce synthesized targeted test command as a blocking verification gate
                    if let Some(ref synth_cmd) = synthesized_verify_cmd_opt {
                        task_to_verify.verification_cmd = Some(synth_cmd.clone());
                    }
                }
            }

            let results = execute_all_gates_filtered(
                &target_dir,
                &task_to_verify,
                verify_git_clean,
                ignore_untracked_glob.as_deref(),
            )
            .map_err(|e| format!("Gate execution error: {}", e))?;

            for res in &results {
                if !res.passed {
                    let reason = res
                        .failure_reason
                        .clone()
                        .unwrap_or_else(|| res.output.clone());
                    state.errors.push(reason.clone());
                    let _ = save_state_atomic(&state_path, &state);

                    return Err(format!(
                        "Verification gate '{}' FAILED:\n{}",
                        res.gate_name, reason
                    ));
                }
            }

            // Resolve base branch from state.base_branch, falling back to manager's current_branch or default_branch
            let base_branch = state.base_branch.clone().unwrap_or_else(|| {
                WorktreeManager::new(&repo_root)
                    .map(|m| m.current_branch().unwrap_or_else(|_| m.default_branch()))
                    .unwrap_or_else(|_| "main".to_string())
            });

            // Merge active worktree into base branch and cleanup
            if let Some(ref wt_path) = state.active_worktree.clone() {
                if wt_path.exists() {
                    let branch = state.active_branch.clone().unwrap_or_else(|| {
                        superstate_worktree::manager::task_worktree_names(
                            &repo_root,
                            &active_task.id,
                            state.base_branch.as_deref(),
                        )
                        .0
                    });
                    let wt = IsolatedWorktree {
                        branch,
                        path: wt_path.clone(),
                        base_branch: base_branch.clone(),
                        repo_root: repo_root.clone(),
                    };
                    let commit_msg = format!("feat({}): {}", active_task.id, active_task.title);
                    wt.merge_and_cleanup(&commit_msg)
                        .map_err(|e| format!("Failed to merge and cleanup worktree: {}", e))?;
                    state.active_worktree = None;
                    state.active_branch = None;
                }
            }

            advance_step(&mut state).map_err(|e| format!("Advance step error: {}", e))?;

            // Provision worktree for the next task if isolation is enabled
            if !state.no_worktree && repo_root.join(".git").exists() {
                if let Some(ref next_task) = state.active_task {
                    let manager = WorktreeManager::new(&repo_root)
                        .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
                    let wt = manager
                        .ensure_task_worktree(&next_task.id, state.base_branch.as_deref())
                        .map_err(|e| {
                            format!("Failed to ensure worktree for task {}: {}", next_task.id, e)
                        })?;
                    state.active_worktree = Some(wt.path.clone());
                    state.active_branch = Some(wt.branch.clone());
                }
            }

            // Render turnkey prompt for the newly active task if one exists
            let mut turnkey_prompt_opt = None;
            if state.active_task.is_some() {
                let is_in_place = state.no_worktree;
                if let Ok(prompt) =
                    render_turnkey_prompt(&state, &state_path, None, is_in_place, false)
                {
                    turnkey_prompt_opt = Some(prompt);
                }
            }

            save_state_atomic(&state_path, &state)
                .map_err(|e| format!("Save state error: {}", e))?;

            let mut res_obj = serde_json::json!({
                "status": "success",
                "step": state.step,
                "stage": verify_stage,
                "active_task": state.active_task,
                "active_worktree": state.active_worktree.as_ref().map(|p| p.display().to_string()),
                "active_branch": state.active_branch,
            });

            // Note impacted tests discovered during milestone verification
            if !impacted_tests_list.is_empty() {
                res_obj["impacted_tests"] = serde_json::json!(impacted_tests_list);
            }

            // Include synthesized targeted test runner command if constructed
            if let Some(ref synth) = synthesized_verify_cmd_opt {
                res_obj["synthesized_verify_cmd"] = serde_json::json!(synth);
            }

            if let Some(ref prompt) = turnkey_prompt_opt {
                res_obj["turnkey_prompt"] = serde_json::Value::String(prompt.clone());
                res_obj["content"] = serde_json::json!([
                    {
                        "type": "text",
                        "text": prompt
                    }
                ]);
            }

            Ok(res_obj)
        }
        "state_prompt" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let state = load_state(&state_path).map_err(|e| format!("Load state error: {}", e))?;
            let task = args.get("task").and_then(|v| v.as_str());
            let with_graph = args
                .get("with_graph")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let no_worktree = args
                .get("no_worktree")
                .and_then(|v| v.as_bool())
                .unwrap_or(state.no_worktree);
            let prompt = render_turnkey_prompt(&state, &state_path, task, no_worktree, with_graph)?;

            Ok(serde_json::json!({
                "status": "success",
                "turnkey_prompt": &prompt,
                "prompt": &prompt,
                "content": [{ "type": "text", "text": &prompt }]
            }))
        }
        "state_note" => {
            let note = args
                .get("note")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'note'".to_string())?;

            let author = args
                .get("author")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");

            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            append_ledger_note(&state_path, LedgerEntry::new(author, note))
                .map_err(|e| format!("Note error: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "note_recorded": true
            }))
        }
        "state_get" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let state = load_state(&state_path).map_err(|e| format!("Load state error: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "state": state,
                "state_path": state_path.display().to_string(),
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&state).unwrap_or_default()
                    }
                ]
            }))
        }
        "state_status" => {
            // Resolve repository context and effective branch-scoped state path
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            // Load execution state and associated epistemic ledger history
            let state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;
            let ledger_entries = read_ledger(&state_path);
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            let daemon_status = resolve_daemon_status(&state_path);
            let daemon_info = daemon_status.as_ref().map(|d| {
                serde_json::json!({
                    "pid": d.pid,
                    "running": d.running,
                    "slug": d.slug,
                    "log_file": d.log_file.display().to_string(),
                })
            });

            if as_json {
                // Machine-readable format: serializes execution state directly matching CLI parity
                let mut val = serde_json::to_value(&state)
                    .map_err(|e| format!("Failed to convert state to JSON value: {}", e))?;
                if let Some(ref d) = daemon_info {
                    if let Value::Object(ref mut map) = val {
                        map.insert("daemon".to_string(), d.clone());
                    }
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
                // Human-readable summary: renders step, task progress, and recent epistemic notes
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
                if let Some(ref d) = daemon_info {
                    ret["daemon"] = d.clone();
                }
                Ok(ret)
            }
        }
        "state_show" => {
            // Resolve repository root and state path based on branch isolation
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
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            if as_json {
                // Provide raw execution state in JSON format matching CLI show --json
                let pretty = serde_json::to_string_pretty(&state)
                    .map_err(|e| format!("Failed to serialize state: {}", e))?;
                let mut val = serde_json::to_value(&state)
                    .map_err(|e| format!("Failed to convert state to JSON value: {}", e))?;
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
                // Markdown executive overview: summarizes active task, worktree, and verification command
                let mut md = String::new();
                md.push_str("# Superstate Execution Status\n");
                md.push_str(&format!(
                    "- **Status:** {:?} (Step {})\n",
                    state.status, state.step
                ));
                if let Some(ref base) = state.base_branch {
                    md.push_str(&format!("- **Base Branch:** `{}`\n", base));
                }
                if let Some(ref task) = state.active_task {
                    md.push_str(&format!("- **Active Task:** {}: {}\n", task.id, task.title));
                    if let Some(ref cmd) = task.verification_cmd {
                        md.push_str(&format!("- **Verification Command:** `{}`\n", cmd));
                    }
                } else {
                    md.push_str("- **Active Task:** None\n");
                }
                if let Some(ref wt) = state.active_worktree {
                    if let Some(ref branch) = state.active_branch {
                        md.push_str(&format!(
                            "- **Active Worktree:** `{}` (branch `{}`)\n",
                            wt.display(),
                            branch
                        ));
                    } else {
                        md.push_str(&format!("- **Active Worktree:** `{}`\n", wt.display()));
                    }
                }

                Ok(serde_json::json!({
                    "status": "success",
                    "markdown": md,
                    "output": md,
                    "text": md,
                    "content": [
                        {
                            "type": "text",
                            "text": md
                        }
                    ]
                }))
            }
        }
        "state_resume" => {
            // Resolve project root and locate state file
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
            let root = if repo_root.join(".git").exists() {
                repo_root
            } else {
                resolve_project_root(&state_path)
            };

            // Query recent git history to ground context for resumed agent session
            let commits = std::process::Command::new("git")
                .args(["log", "--oneline", "-5"])
                .current_dir(&root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let notes = read_ledger(&state_path);
            let briefing = resume_briefing(&state, &notes, &commits);

            Ok(serde_json::json!({
                "status": "success",
                "briefing": briefing,
                "output": briefing,
                "text": briefing,
                "content": [
                    {
                        "type": "text",
                        "text": briefing
                    }
                ]
            }))
        }
        "state_metrics" => {
            // Resolve state path and load execution state
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
            let notes = read_ledger(&state_path);

            // Compute performance and completion telemetry metrics
            let m = metrics(&state, notes.len());
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            if as_json {
                let pretty = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
                let mut val = serde_json::to_value(&m)
                    .map_err(|e| format!("Failed to convert metrics to JSON value: {}", e))?;
                if let Value::Object(ref mut map) = val {
                    map.insert(
                        "metrics".to_string(),
                        serde_json::to_value(&m).unwrap_or_default(),
                    );
                    map.insert(
                        "content".to_string(),
                        serde_json::json!([{ "type": "text", "text": pretty }]),
                    );
                }
                Ok(val)
            } else {
                let mut text = String::new();
                text.push_str(&format!("Step:            {}\n", m.step));
                text.push_str(&format!("Status:          {}\n", m.status));
                text.push_str(&format!(
                    "Tasks:           {}/{} complete ({}%)\n",
                    m.tasks_completed, m.tasks_total, m.completion_pct
                ));
                text.push_str(&format!("Remaining:       {}\n", m.tasks_remaining));
                text.push_str(&format!("Notes recorded:  {}\n", m.notes_recorded));
                text.push_str(&format!("Errors recorded: {}\n", m.errors_recorded));

                Ok(serde_json::json!({
                    "status": "success",
                    "metrics": m,
                    "output": text,
                    "text": text,
                    "content": [
                        {
                            "type": "text",
                            "text": text
                        }
                    ]
                }))
            }
        }
        "state_checkpoint" => {
            // Resolve repository root and state path
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            // Load execution state to determine current step
            let state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;
            let root = repo_root_for(&state_path, &repo_root);

            // Create git checkpoint tag guarding this step
            let tag = superstate_core::checkpoint::create_checkpoint(&root, state.step)
                .map_err(|e| format!("Failed to create checkpoint: {}", e))?;

            Ok(serde_json::json!({
                "checkpoint": tag,
                "step": state.step,
                "content": [
                    {
                        "type": "text",
                        "text": format!("Checkpoint created: {}", tag)
                    }
                ]
            }))
        }
        "state_rollback" => {
            // Resolve repository root and state path
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            // Load state to inspect worktree and step
            let mut state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;
            let root = repo_root_for(&state_path, &repo_root);

            // If an isolated worktree was active for this step, abort it cleanly before restoring
            // the git checkpoint, preventing dangling branches or worktrees.
            if let Some(ref wt_path) = state.active_worktree.clone() {
                if wt_path.exists() {
                    let base_branch = WorktreeManager::new(&root)
                        .map(|m| m.default_branch())
                        .unwrap_or_else(|_| "main".to_string());
                    let branch = state
                        .active_branch
                        .clone()
                        .unwrap_or_else(|| "wt/task".to_string());
                    let wt = IsolatedWorktree {
                        branch,
                        path: wt_path.clone(),
                        base_branch,
                        repo_root: root.clone(),
                    };
                    wt.abort()
                        .map_err(|e| format!("Failed to abort worktree on rollback: {}", e))?;
                }
                state.active_worktree = None;
                state.active_branch = None;
                save_state_atomic(&state_path, &state)
                    .map_err(|e| format!("Failed to update state after worktree abort: {}", e))?;
            }

            let reference = args.get("reference").and_then(Value::as_str);
            let preserved_paths = preserved(&state_path);
            let preserve_refs: Vec<&str> = preserved_paths.iter().map(String::as_str).collect();

            // Perform hard reset and clean untracked debris while preserving state file
            let target = superstate_core::checkpoint::rollback_to(
                &root,
                state.step,
                reference,
                &preserve_refs,
            )
            .map_err(|e| format!("Failed to roll back: {}", e))?;

            Ok(serde_json::json!({
                "target": target,
                "step": state.step,
                "content": [
                    {
                        "type": "text",
                        "text": format!("Working tree cleanly rolled back to {}", target)
                    }
                ]
            }))
        }
        "state_exec" => {
            let verify_cmd = args
                .get("verify_cmd")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing required argument 'verify_cmd'".to_string())?;

            // Resolve repository context and effective branch-scoped state path
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let mut state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;
            let root = repo_root_for(&state_path, &repo_root);
            let step = state.step;

            let checkpoint = args
                .get("checkpoint")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let rollback_on_fail = args
                .get("rollback_on_fail")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let advance = args.get("advance").and_then(Value::as_bool).unwrap_or(true);
            let verify_git_clean = args
                .get("verify_git_clean")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            // Optional checkpoint creation before running verification command
            if checkpoint {
                superstate_core::checkpoint::create_checkpoint(&root, step)
                    .map_err(|e| format!("Failed to checkpoint before exec: {}", e))?;
            }

            // Target directory: active worktree if present, else repository root
            let target_dir = state
                .active_worktree
                .as_ref()
                .filter(|p| p.exists())
                .cloned()
                .unwrap_or_else(|| root.clone());

            // Run verification command with standard 600s timeout
            let gate =
                superstate_core::verifier::run_verification_cmd(verify_cmd, &target_dir, 600)
                    .map_err(|e| format!("Verification could not run: {}", e))?;

            // Check git clean status if requested and verification command passed
            let mut git_clean_passed = true;
            if gate.passed && verify_git_clean {
                ensure_git_exclude(&root);
                let clean = superstate_core::verifier::verify_git_clean(&target_dir)
                    .map_err(|e| format!("git clean check failed: {}", e))?;
                git_clean_passed = clean.passed;
            }

            let passed = gate.passed && git_clean_passed;
            let mut rolled_back = false;
            let mut advanced = false;

            if passed && advance {
                // Merge active worktree into base branch and cleanup if present
                if let Some(ref wt_path) = state.active_worktree.clone() {
                    if wt_path.exists() {
                        let base_branch = state.base_branch.clone().unwrap_or_else(|| {
                            WorktreeManager::new(&root)
                                .map(|m| m.current_branch().unwrap_or_else(|_| m.default_branch()))
                                .unwrap_or_else(|_| "main".to_string())
                        });
                        let branch = state.active_branch.clone().unwrap_or_else(|| {
                            superstate_worktree::manager::task_worktree_names(
                                &root,
                                state
                                    .active_task
                                    .as_ref()
                                    .map(|t| t.id.as_str())
                                    .unwrap_or("task"),
                                state.base_branch.as_deref(),
                            )
                            .0
                        });
                        let wt = IsolatedWorktree {
                            branch,
                            path: wt_path.clone(),
                            base_branch,
                            repo_root: root.clone(),
                        };
                        let commit_msg = format!(
                            "feat({}): {}",
                            state
                                .active_task
                                .as_ref()
                                .map(|t| t.id.as_str())
                                .unwrap_or("task"),
                            state
                                .active_task
                                .as_ref()
                                .map(|t| t.title.as_str())
                                .unwrap_or("task"),
                        );
                        wt.merge_and_cleanup(&commit_msg)
                            .map_err(|e| format!("Failed to merge and cleanup worktree: {}", e))?;
                        state.active_worktree = None;
                        state.active_branch = None;
                    }
                }

                advance_step(&mut state).map_err(|e| format!("Advance step error: {}", e))?;

                // Provision worktree for the next task if isolation is enabled
                if !state.no_worktree && root.join(".git").exists() {
                    if let Some(ref next_task) = state.active_task {
                        let manager = WorktreeManager::new(&root)
                            .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
                        let wt = manager
                            .ensure_task_worktree(&next_task.id, state.base_branch.as_deref())
                            .map_err(|e| {
                                format!(
                                    "Failed to ensure worktree for task {}: {}",
                                    next_task.id, e
                                )
                            })?;
                        state.active_worktree = Some(wt.path.clone());
                        state.active_branch = Some(wt.branch.clone());
                    }
                }

                save_state_atomic(&state_path, &state)
                    .map_err(|e| format!("Save state error: {}", e))?;
                advanced = true;
            } else if !passed {
                // Record why, so the failure survives in the state
                let mut failed_state = load_state(&state_path)
                    .map_err(|e| format!("Failed to reload state: {}", e))?;
                failed_state.errors.push(format!(
                    "step {}: verification failed for `{}`: {}",
                    step,
                    verify_cmd,
                    gate.failure_reason.clone().unwrap_or_else(|| {
                        if git_clean_passed {
                            "non-zero exit".into()
                        } else {
                            "working tree not clean".into()
                        }
                    })
                ));
                save_state_atomic(&state_path, &failed_state)
                    .map_err(|e| format!("Failed to save state after error: {}", e))?;

                if rollback_on_fail {
                    // Abort isolated worktree if active
                    if let Some(ref wt_path) = failed_state.active_worktree {
                        if wt_path.exists() {
                            let base_branch = WorktreeManager::new(&root)
                                .map(|m| m.default_branch())
                                .unwrap_or_else(|_| "main".to_string());
                            let branch = failed_state.active_branch.clone().unwrap_or_else(|| {
                                format!(
                                    "wt/{}",
                                    superstate_worktree::manager::sanitize_slug(
                                        failed_state
                                            .active_task
                                            .as_ref()
                                            .map(|t| t.id.as_str())
                                            .unwrap_or("task"),
                                    )
                                )
                            });
                            let wt = IsolatedWorktree {
                                branch,
                                path: wt_path.clone(),
                                base_branch,
                                repo_root: root.clone(),
                            };
                            let _ = wt.abort();
                            failed_state.active_worktree = None;
                            failed_state.active_branch = None;
                            let _ = save_state_atomic(&state_path, &failed_state);
                        }
                    }
                    let preserved_paths = preserved(&state_path);
                    let preserve_refs: Vec<&str> =
                        preserved_paths.iter().map(String::as_str).collect();
                    superstate_core::checkpoint::rollback_to(&root, step, None, &preserve_refs)
                        .map_err(|e| format!("Failed to roll back after failure: {}", e))?;
                    rolled_back = true;
                }
            }

            let text = if passed {
                format!(
                    "Verification passed{}",
                    if advanced {
                        " and state advanced."
                    } else {
                        "."
                    }
                )
            } else {
                format!(
                    "Verification failed{}",
                    if rolled_back {
                        "; working tree rolled back."
                    } else {
                        "; failure recorded in state."
                    }
                )
            };

            let mut res = serde_json::json!({
                "passed": passed,
                "advanced": advanced,
                "rolled_back": rolled_back,
                "step": step,
                "verify_cmd": verify_cmd,
                "output": gate.output,
            });

            if as_json {
                let pretty = serde_json::to_string_pretty(&res).unwrap_or_default();
                res["content"] = serde_json::json!([{ "type": "text", "text": pretty }]);
            } else {
                res["text"] = serde_json::Value::String(text.clone());
                res["content"] = serde_json::json!([{ "type": "text", "text": text }]);
            }

            Ok(res)
        }
        "superstate_review" => {
            let repo_root = args
                .get("repo")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let commit_range = args.get("commit_range").and_then(|v| v.as_str());
            let diff =
                superstate_review::get_git_diff(&repo_root, commit_range).unwrap_or_default();
            let prompt = superstate_review::build_review_system_prompt();

            Ok(serde_json::json!({
                "diff_lines": diff.lines().count(),
                "review_prompt": prompt,
                "diff": diff
            }))
        }
        "worktree_create" => {
            let branch = args
                .get("branch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'branch'".to_string())?;

            let repo_root = args
                .get("repo")
                .or_else(|| args.get("repo_root"))
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let base = args.get("base").and_then(|v| v.as_str());
            let custom_path = args.get("path").and_then(|v| v.as_str()).map(PathBuf::from);

            ensure_git_exclude(&repo_root);
            let manager = WorktreeManager::new(&repo_root)
                .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;

            let config = WorktreeConfig {
                base_path: repo_root.clone(),
                branch: branch.to_string(),
                base_branch: base.map(ToString::to_string),
                worktree_path: custom_path,
                cache_dirs: vec![],
            };

            let isolated = manager
                .create(&config)
                .map_err(|e| format!("Failed to create worktree: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "branch": isolated.branch,
                "worktree_path": isolated.path.display().to_string(),
            }))
        }
        "worktree_merge" => {
            let branch = args
                .get("branch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'branch'".to_string())?;

            let repo_root = args
                .get("repo")
                .or_else(|| args.get("repo_root"))
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let base = args.get("base").and_then(|v| v.as_str());
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Merge worktree branch");
            let verify_full_suite = args
                .get("verify_full_suite")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());

            let isolated = resolve_worktree_for_merge(&repo_root, branch, base)?;
            let _ = isolated.auto_commit_if_dirty(message);

            // Enforce pre-merge verification gate if a full suite check command is specified.
            // If verification fails (non-zero exit), abort before merging into base so the
            // isolated worktree remains intact for debugging and regressions are kept out of base.
            if let Some(cmd) = verify_full_suite {
                let gate_res = execute_preflight_with_timeout(
                    cmd,
                    &isolated.path,
                    std::time::Duration::from_secs(600),
                )?;
                if gate_res.exit_code != 0 {
                    return Err(format!(
                        "Pre-merge verification failed (exit code {}):\nSTDOUT:\n{}\nSTDERR:\n{}",
                        gate_res.exit_code, gate_res.stdout, gate_res.stderr
                    ));
                }
            }

            isolated
                .merge_into_base(message)
                .map_err(|e| format!("Failed to merge worktree: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "branch": branch,
                "base": isolated.base_branch,
            }))
        }
        "worktree_list" => {
            let repo_root = args
                .get("repo")
                .or_else(|| args.get("repo_root"))
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            let manager = WorktreeManager::new(&repo_root)
                .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
            let list = manager
                .list()
                .map_err(|e| format!("Failed to list worktrees: {}", e))?;

            let json_list: Vec<Value> = list
                .iter()
                .map(|wt| {
                    serde_json::json!({
                        "branch": wt.branch,
                        "path": wt.path.display().to_string(),
                        "head": wt.commit_sha,
                        "commit_sha": wt.commit_sha,
                        "is_main": wt.is_main,
                        "active": wt.is_main,
                    })
                })
                .collect();

            let mut text = String::new();
            if list.is_empty() {
                text.push_str("No active worktrees.\n");
            } else {
                text.push_str(&format!("{:<25} {:<10} {:<40}\n", "BRANCH", "HEAD", "PATH"));
                text.push_str(&format!("{:-<25} {:-<10} {:-<40}\n", "", "", ""));
                for wt in &list {
                    let branch_display = wt
                        .branch
                        .clone()
                        .unwrap_or_else(|| "(detached HEAD)".to_string())
                        + if wt.is_main { " [main]" } else { "" };
                    let short_sha = if wt.commit_sha.len() >= 7 {
                        &wt.commit_sha[..7]
                    } else {
                        &wt.commit_sha
                    };
                    text.push_str(&format!(
                        "{:<25} {:<10} {:<40}\n",
                        branch_display,
                        short_sha,
                        wt.path.display()
                    ));
                }
            }

            if as_json {
                let pretty = serde_json::to_string_pretty(&json_list)
                    .map_err(|e| format!("Failed to serialize worktree list: {}", e))?;
                Ok(serde_json::json!({
                    "status": "success",
                    "worktrees": json_list,
                    "content": [{
                        "type": "text",
                        "text": pretty
                    }]
                }))
            } else {
                Ok(serde_json::json!({
                    "status": "success",
                    "worktrees": json_list,
                    "output": text,
                    "text": text,
                    "content": [{
                        "type": "text",
                        "text": text
                    }]
                }))
            }
        }
        "worktree_remove" => {
            let branch = args
                .get("branch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'branch'".to_string())?;

            let repo_root = args
                .get("repo")
                .or_else(|| args.get("repo_root"))
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);

            let isolated = resolve_worktree_for_remove(&repo_root, branch)?;
            let wt_path = isolated.path.clone();
            isolated
                .destroy(force)
                .map_err(|e| format!("Failed to remove worktree: {}", e))?;

            if wt_path.exists() {
                let _ = std::fs::remove_dir_all(&wt_path);
            }

            Ok(serde_json::json!({
                "status": "success",
                "branch": branch,
                "removed": true,
                "content": [{
                    "type": "text",
                    "text": format!("Removed worktree for branch '{}'", branch)
                }]
            }))
        }
        "worktree_clean" => {
            let repo_root = args
                .get("repo")
                .or_else(|| args.get("repo_root"))
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);

            let (cleaned_count, cleaned_branches) = clean_orphaned_worktrees(&repo_root, force)
                .map_err(|e| format!("Failed to clean worktrees: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "cleaned": cleaned_count,
                "branches": cleaned_branches,
                "content": [{
                    "type": "text",
                    "text": format!("Cleaned {} orphaned worktree(s)", cleaned_count)
                }]
            }))
        }
        "state_ready" => {
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
            let ready = ready_tasks(&state);
            let ready_titles: Vec<String> = ready
                .iter()
                .map(|t| format!("{}: {}", t.id, t.title))
                .collect();
            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            let text = if ready.is_empty() {
                "No tasks are currently unblocked (all remaining tasks have unsatisfied dependencies).".to_string()
            } else {
                let mut out = format!("Unblocked tasks ({}):\n", ready.len());
                for task in &ready {
                    out.push_str(&format!("  - {}: {}\n", task.id, task.title));
                }
                out
            };

            if as_json {
                let pretty =
                    serde_json::to_string_pretty(&serde_json::json!({ "ready": &ready_titles }))
                        .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({
                    "status": "success",
                    "ready": ready_titles,
                    "content": [{ "type": "text", "text": pretty }]
                }))
            } else {
                Ok(serde_json::json!({
                    "status": "success",
                    "ready": ready_titles,
                    "output": text,
                    "text": text,
                    "content": [{ "type": "text", "text": text }]
                }))
            }
        }
        "state_sync" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let plan_str = args
                .get("plan")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing required parameter 'plan' for state_sync".to_string())?;

            let plan_path = if Path::new(plan_str).is_absolute() {
                PathBuf::from(plan_str)
            } else {
                repo_root.join(plan_str)
            };

            let plan_content = std::fs::read_to_string(&plan_path)
                .map_err(|e| format!("Failed to read plan at {}: {}", plan_path.display(), e))?;

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let mut state =
                load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?;

            let report = sync_with_plan(&mut state, &plan_content)?;
            state.plan_path = Some(plan_path.clone());

            let target_path = if let Some(out_str) = args.get("out").and_then(Value::as_str) {
                if Path::new(out_str).is_absolute() {
                    PathBuf::from(out_str)
                } else {
                    repo_root.join(out_str)
                }
            } else {
                state_path.clone()
            };

            save_state_atomic(&target_path, &state)
                .map_err(|e| format!("Failed to write state: {}", e))?;

            let mut text = format!("State synced with plan '{}':\n", plan_path.display());
            for a in &report.added {
                text.push_str(&format!("  + {}\n", a));
            }
            for r in &report.removed {
                text.push_str(&format!("  - {}\n", r));
            }
            if report.preserved_completed > 0 {
                text.push_str(&format!(
                    "  {} completed task(s) preserved.\n",
                    report.preserved_completed
                ));
            }
            if let Some(ref active) = report.active_task {
                text.push_str(&format!("Active: {}\n", active));
            }
            text.push_str(&format!("Pending: {}\n", report.pending));

            Ok(serde_json::json!({
                "status": "success",
                "report": &report,
                "added": &report.added,
                "removed": &report.removed,
                "preserved_completed": report.preserved_completed,
                "active_task": &report.active_task,
                "pending": report.pending,
                "output": text,
                "text": text,
                "content": [{ "type": "text", "text": text }]
            }))
        }
        "state_doctor" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

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
        "state_validate" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let body = std::fs::read_to_string(&state_path)
                .map_err(|e| format!("Failed to read {}: {}", state_path.display(), e))?;
            let raw: Value = serde_json::from_str(&body)
                .map_err(|e| format!("Failed to parse {}: {}", state_path.display(), e))?;

            let errors = validate_state(&raw);
            if errors.is_empty() {
                Ok(serde_json::json!({
                    "status": "success",
                    "valid": true,
                    "errors": Vec::<String>::new(),
                    "output": "State is valid.",
                    "text": "State is valid.",
                    "content": [{ "type": "text", "text": "State is valid." }]
                }))
            } else {
                let msg = format!("State is invalid ({} problem(s)).", errors.len());
                let mut text = format!("State is invalid ({} problem(s)):\n", errors.len());
                for err in &errors {
                    text.push_str(&format!("  - {}\n", err));
                }
                Ok(serde_json::json!({
                    "status": "error",
                    "valid": false,
                    "errors": errors,
                    "output": text,
                    "text": text,
                    "content": [{ "type": "text", "text": msg }]
                }))
            }
        }
        "state_apply" => {
            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let patch_val =
                if let Some(p_val) = args.get("patch") {
                    if let Some(s) = p_val.as_str() {
                        let candidate = if Path::new(s).is_absolute() {
                            PathBuf::from(s)
                        } else {
                            repo_root.join(s)
                        };
                        if candidate.is_file() {
                            let content = std::fs::read_to_string(&candidate).map_err(|e| {
                                format!("Failed to read patch file {}: {}", candidate.display(), e)
                            })?;
                            Some(serde_json::from_str::<Value>(&content).map_err(|e| {
                                format!("Failed to parse patch file as JSON: {}", e)
                            })?)
                        } else {
                            Some(serde_json::from_str::<Value>(s).map_err(|e| {
                                format!("Failed to parse patch string as JSON: {}", e)
                            })?)
                        }
                    } else if p_val.is_object() {
                        Some(p_val.clone())
                    } else {
                        return Err(
                            "Invalid 'patch' argument: must be JSON object or string".to_string()
                        );
                    }
                } else {
                    None
                };

            let note = args.get("note").and_then(Value::as_str);

            if patch_val.is_none() && note.is_none() {
                return Err("Nothing to apply: pass patch and/or note.".to_string());
            }

            let state_path =
                resolve_mcp_state_path(&repo_root, args.get("state").and_then(Value::as_str));

            let body = std::fs::read_to_string(&state_path)
                .map_err(|e| format!("Failed to read {}: {}", state_path.display(), e))?;
            let mut state_val: Value = serde_json::from_str(&body)
                .map_err(|e| format!("Failed to parse {}: {}", state_path.display(), e))?;

            if let Some(ref p) = patch_val {
                apply_patch(&mut state_val, p);
            }

            if let Some(n) = note {
                append_ledger_note(&state_path, LedgerEntry::new("agent", n))
                    .map_err(|e| format!("Failed to record note: {}", e))?;
            }

            let errors = validate_state(&state_val);
            if !errors.is_empty() {
                return Err(format!(
                    "Patch would produce an invalid state; nothing written ({} problem(s)): {}",
                    errors.len(),
                    errors.join(", ")
                ));
            }

            let save_path = if let Some(out_str) = args.get("out").and_then(Value::as_str) {
                if Path::new(out_str).is_absolute() {
                    PathBuf::from(out_str)
                } else {
                    repo_root.join(out_str)
                }
            } else {
                state_path.clone()
            };

            save_json_atomic(&save_path, &state_val)?;

            Ok(serde_json::json!({
                "status": "success",
                "applied": true,
                "note": note,
                "output": "State updated.",
                "text": "State updated.",
                "content": [{ "type": "text", "text": "State updated." }]
            }))
        }
        "state_list" => {
            let repo_root = if let Some(arg) = args.get("state").and_then(Value::as_str) {
                let p = if Path::new(arg).is_absolute() {
                    PathBuf::from(arg)
                } else {
                    let base = args
                        .get("repo_root")
                        .or_else(|| args.get("cwd"))
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                        .unwrap_or_else(|| {
                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                        });
                    base.join(arg)
                };
                if p.is_dir() {
                    if p.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
                        p.parent().unwrap_or(&p).to_path_buf()
                    } else if p.file_name().and_then(|s| s.to_str()) == Some("branches")
                        && p.parent()
                            .and_then(|pp| pp.file_name())
                            .and_then(|s| s.to_str())
                            == Some(STATE_DIR)
                    {
                        p.parent()
                            .and_then(|pp| pp.parent())
                            .unwrap_or(&p)
                            .to_path_buf()
                    } else {
                        p
                    }
                } else {
                    superstate_core::prompt::resolve_project_root(&p)
                }
            } else {
                args.get("repo_root")
                    .or_else(|| args.get("cwd"))
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    })
            };

            let state_dir = repo_root.join(STATE_DIR);
            let mut summaries = Vec::new();

            // 1. Scan .agent-state/branches/*.json
            let branches_dir = state_dir.join("branches");
            if branches_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&branches_dir) {
                    let mut paths: Vec<PathBuf> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| {
                            p.is_file()
                                && p.extension().and_then(|ext| ext.to_str()) == Some("json")
                        })
                        .collect();
                    paths.sort();

                    for path in paths {
                        if let Ok(st) = load_state(&path) {
                            let slug = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let total_steps = st.completed_tasks.len()
                                + if st.active_task.is_some() { 1 } else { 0 }
                                + st.pending_tasks.len();
                            let active_task = st
                                .active_task
                                .as_ref()
                                .map(|t| format!("{}: {}", t.id, t.title));
                            let display_step = if st.status == ExecutionStatus::Completed {
                                total_steps
                            } else {
                                st.step.min(total_steps)
                            };
                            let daemon = resolve_daemon_status(&path);
                            summaries.push(BranchStateSummary {
                                branch: slug,
                                status: st.status,
                                active_task,
                                step: display_step,
                                total_steps,
                                base_branch: st.base_branch,
                                state_file: path,
                                daemon,
                            });
                        }
                    }
                }
            }

            // 2. Scan .agent-state/state.json if present
            let default_state_file = state_dir.join("state.json");
            if default_state_file.is_file() {
                if let Ok(st) = load_state(&default_state_file) {
                    let total_steps = st.completed_tasks.len()
                        + if st.active_task.is_some() { 1 } else { 0 }
                        + st.pending_tasks.len();
                    let active_task = st
                        .active_task
                        .as_ref()
                        .map(|t| format!("{}: {}", t.id, t.title));
                    let default_branch_name = WorktreeManager::new(&repo_root)
                        .map(|m| m.default_branch())
                        .unwrap_or_else(|_| "main".to_string());
                    let branch_display = format!("{} (default)", default_branch_name);
                    let display_step = if st.status == ExecutionStatus::Completed {
                        total_steps
                    } else {
                        st.step.min(total_steps)
                    };
                    let daemon = resolve_daemon_status(&default_state_file);
                    summaries.push(BranchStateSummary {
                        branch: branch_display,
                        status: st.status,
                        active_task,
                        step: display_step,
                        total_steps,
                        base_branch: st.base_branch.or(Some(default_branch_name)),
                        state_file: default_state_file,
                        daemon,
                    });
                }
            }

            // 3. Scan .wt/* worktree directories if repo exists
            let wt_dir = repo_root.join(".wt");
            if wt_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&wt_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let p = entry.path();
                        if p.is_dir() {
                            let wt_state = p.join(STATE_DIR).join("state.json");
                            if wt_state.is_file()
                                && !summaries.iter().any(|s| s.state_file == wt_state)
                            {
                                if let Ok(st) = load_state(&wt_state) {
                                    let wt_name =
                                        p.file_name().and_then(|s| s.to_str()).unwrap_or("wt");
                                    let total_steps = st.completed_tasks.len()
                                        + if st.active_task.is_some() { 1 } else { 0 }
                                        + st.pending_tasks.len();
                                    let active_task = st
                                        .active_task
                                        .as_ref()
                                        .map(|t| format!("{}: {}", t.id, t.title));
                                    let display_step = if st.status == ExecutionStatus::Completed {
                                        total_steps
                                    } else {
                                        st.step.min(total_steps)
                                    };
                                    let daemon = resolve_daemon_status(&wt_state);
                                    summaries.push(BranchStateSummary {
                                        branch: format!("{} (worktree)", wt_name),
                                        status: st.status,
                                        active_task,
                                        step: display_step,
                                        total_steps,
                                        base_branch: st.base_branch,
                                        state_file: wt_state,
                                        daemon,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            summaries.sort_by(|a, b| a.branch.cmp(&b.branch));

            let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

            let summaries_val: Vec<Value> = summaries
                .iter()
                .map(|s| {
                    let mut obj = serde_json::json!({
                        "branch": s.branch,
                        "status": format!("{:?}", s.status),
                        "active_task": s.active_task,
                        "step": s.step,
                        "total_steps": s.total_steps,
                        "base_branch": s.base_branch,
                        "state_file": s.state_file.display().to_string(),
                    });
                    if let Some(ref d) = s.daemon {
                        obj["daemon"] = serde_json::json!({
                            "pid": d.pid,
                            "running": d.running,
                            "slug": d.slug,
                            "log_file": d.log_file.display().to_string(),
                        });
                    } else {
                        obj["daemon"] = Value::Null;
                    }
                    obj
                })
                .collect();

            let mut text = String::from("Active & Completed Branches:\n");
            text.push_str(&format!(
                "  {:<26} {:<12} {:<24} {:<6} {:<20} {:<16}\n",
                "Branch", "Status", "Active Task", "Step", "Base Branch", "Daemon"
            ));
            for s in &summaries {
                let status_str = match s.status {
                    ExecutionStatus::NotStarted => "NotStarted",
                    ExecutionStatus::InProgress => "InProgress",
                    ExecutionStatus::Completed => "Completed",
                    ExecutionStatus::Failed => "Failed",
                    ExecutionStatus::Blocked => "Blocked",
                };
                let task_str = s.active_task.as_deref().unwrap_or("-");
                let step_str = format!("{}/{}", s.step, s.total_steps);
                let base_str = s.base_branch.as_deref().unwrap_or("-");
                let daemon_str = match &s.daemon {
                    Some(d) if d.running => format!("Running ({})", d.pid),
                    Some(d) => format!("Stopped ({})", d.pid),
                    None => "-".to_string(),
                };
                text.push_str(&format!(
                    "  {:<26} {:<12} {:<24} {:<6} {:<20} {:<16}\n",
                    s.branch,
                    status_str,
                    if task_str.len() > 22 {
                        &task_str[..22]
                    } else {
                        task_str
                    },
                    step_str,
                    base_str,
                    daemon_str
                ));
            }

            if as_json {
                let pretty = serde_json::to_string_pretty(&summaries_val)
                    .map_err(|e| format!("Failed to serialize summaries: {}", e))?;
                Ok(serde_json::json!({
                    "status": "success",
                    "states": summaries_val,
                    "branches": summaries_val,
                    "content": [{ "type": "text", "text": pretty }]
                }))
            } else {
                Ok(serde_json::json!({
                    "status": "success",
                    "states": summaries_val,
                    "branches": summaries_val,
                    "output": text,
                    "text": text,
                    "content": [{ "type": "text", "text": text }]
                }))
            }
        }
        unknown => Err(format!("Unknown tool: {}", unknown)),
    }
}

fn resolve_worktree_for_merge(
    repo: &Path,
    branch: &str,
    expected_base: Option<&str>,
) -> Result<IsolatedWorktree, String> {
    let manager = WorktreeManager::new(repo).map_err(|e| e.to_string())?;
    let list = manager.list().map_err(|e| e.to_string())?;

    let primary_wt = list
        .iter()
        .find(|w| w.is_main)
        .cloned()
        .ok_or_else(|| "Could not identify primary repository worktree".to_string())?;

    let wt = list
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
        .ok_or_else(|| format!("Worktree branch '{}' not found", branch))?;

    if wt.is_main {
        return Err(format!("Cannot merge primary worktree branch '{}'", branch));
    }

    let base_branch = expected_base
        .map(|s| s.to_string())
        .or_else(|| primary_wt.branch.clone())
        .unwrap_or_else(|| "main".to_string());

    if primary_wt.branch.is_none() {
        return Err(
            "Cannot merge: primary repository worktree is in a detached HEAD state".to_string(),
        );
    }
    if primary_wt.branch.as_deref() != Some(&base_branch) {
        return Err(format!(
            "Primary worktree is checked out to {:?}, expected target base branch '{}'",
            primary_wt.branch, base_branch
        ));
    }

    Ok(IsolatedWorktree {
        branch: branch.to_string(),
        path: wt.path,
        base_branch,
        repo_root: primary_wt.path,
    })
}

fn resolve_worktree_for_remove(repo: &Path, branch: &str) -> Result<IsolatedWorktree, String> {
    let manager = WorktreeManager::new(repo).map_err(|e| e.to_string())?;
    let list = manager.list().map_err(|e| e.to_string())?;

    let primary_wt = list
        .iter()
        .find(|w| w.is_main)
        .cloned()
        .ok_or_else(|| "Could not identify primary repository worktree".to_string())?;

    let wt = list
        .into_iter()
        .find(|w| {
            w.branch.as_deref() == Some(branch)
                || w.branch.as_deref() == Some(&format!("wt/{}", branch))
                || branch
                    .strip_prefix("wt/")
                    .map(|b| w.branch.as_deref() == Some(b))
                    .unwrap_or(false)
        })
        .ok_or_else(|| format!("Worktree branch '{}' not found", branch))?;

    if wt.is_main {
        return Err(format!(
            "Cannot remove primary worktree branch '{}'",
            branch
        ));
    }

    if let Ok(cwd) = std::env::current_dir() {
        let is_inside = match (cwd.canonicalize(), wt.path.canonicalize()) {
            (Ok(c), Ok(w)) => c.starts_with(&w),
            _ => cwd.starts_with(&wt.path),
        };
        if is_inside {
            return Err(format!(
                "Cannot remove worktree '{}' while current working directory is inside it",
                branch
            ));
        }
    }

    let actual_branch = wt.branch.unwrap_or_else(|| branch.to_string());
    let base_branch = primary_wt
        .branch
        .clone()
        .unwrap_or_else(|| "main".to_string());

    Ok(IsolatedWorktree {
        branch: actual_branch,
        path: wt.path,
        base_branch,
        repo_root: primary_wt.path,
    })
}

fn clean_orphaned_worktrees(repo_root: &Path, force: bool) -> Result<(usize, Vec<String>), String> {
    let mut cleaned_count = 0;
    let mut cleaned_branches = Vec::new();

    // 1. Run git worktree prune to eliminate stale worktree references in git metadata
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output();

    // 2. Fetch active worktree list from WorktreeManager
    let manager = WorktreeManager::new(repo_root).map_err(|e| e.to_string())?;
    let active_worktrees = manager.list().unwrap_or_default();

    let mut active_paths: Vec<PathBuf> = Vec::new();
    let mut active_branches: Vec<String> = Vec::new();

    for wt in &active_worktrees {
        if let Ok(canon) = wt.path.canonicalize() {
            active_paths.push(canon);
        } else {
            active_paths.push(wt.path.clone());
        }
        if let Some(ref b) = wt.branch {
            active_branches.push(b.clone());
        }
    }

    let del_flag = if force { "-D" } else { "-d" };

    // 3. Inspect .wt/ directory in the repository for orphaned directories
    let wt_dir = repo_root.join(".wt");
    if wt_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&wt_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
                    if !active_paths.contains(&canon) {
                        // Orphaned worktree directory
                        let dir_name = entry.file_name().to_string_lossy().to_string();
                        // Remove directory from disk
                        let _ = std::fs::remove_dir_all(&path);

                        // Check if an orphaned branch corresponding to this dir exists
                        let mut candidate_branches =
                            vec![format!("wt/{}", dir_name), dir_name.clone()];
                        if let Some(stripped) = dir_name.strip_prefix("wt-") {
                            candidate_branches.push(format!("wt/{}", stripped));
                            candidate_branches.push(stripped.to_string());
                        }
                        let mut branch_deleted = false;
                        for cand in &candidate_branches {
                            if !active_branches.contains(cand) {
                                let check = std::process::Command::new("git")
                                    .args(["rev-parse", "--verify", cand])
                                    .current_dir(repo_root)
                                    .output();
                                if let Ok(out) = check {
                                    if out.status.success() {
                                        let del = std::process::Command::new("git")
                                            .args(["branch", del_flag, cand])
                                            .current_dir(repo_root)
                                            .output();
                                        if let Ok(del_out) = del {
                                            if del_out.status.success()
                                                && !cleaned_branches.contains(cand)
                                            {
                                                cleaned_branches.push(cand.clone());
                                                branch_deleted = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if !branch_deleted && !cleaned_branches.contains(&dir_name) {
                            cleaned_branches.push(dir_name);
                        }
                        cleaned_count += 1;
                    }
                }
            }
        }
    }

    // 4. Inspect git branches starting with wt/ that have no active worktree
    let branch_out = std::process::Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/wt/*",
        ])
        .current_dir(repo_root)
        .output();

    if let Ok(out) = branch_out {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            let branch = line.trim();
            if branch.is_empty() {
                continue;
            }
            if !active_branches.iter().any(|ab| ab == branch) {
                // Delete orphaned branch
                let del_out = std::process::Command::new("git")
                    .args(["branch", del_flag, branch])
                    .current_dir(repo_root)
                    .output();
                if let Ok(del) = del_out {
                    if del.status.success() {
                        let branch_str = branch.to_string();
                        if !cleaned_branches.contains(&branch_str) {
                            cleaned_branches.push(branch_str);
                            cleaned_count += 1;
                        }
                    }
                }
            }
        }
    }

    // 5. Run git worktree prune one more time
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output();

    Ok((cleaned_count, cleaned_branches))
}

/// Synthesizes a targeted test runner command based on inferred test file paths.
///
/// Discovers the project test language family (Python, Rust, JavaScript/TypeScript, Go)
/// from the provided test files and formats a targeted runner invocation referencing
/// only the impacted test files.
///
/// # Parameters
/// - `repo_root`: Repository root path used to relativize test file paths.
/// - `tests`: Slice of absolute or relative test file paths to execute.
///
/// # Returns
/// An `Option<String>` containing the synthesized runner command, or `None` if
/// `tests` is empty or no valid files can be referenced.
fn synthesize_targeted_test_command(repo_root: &Path, tests: &[PathBuf]) -> Option<String> {
    // Guard clause: Return early if no candidate test files exist
    if tests.is_empty() {
        return None;
    }

    // Relativize all candidate paths against repo_root for clean command line strings
    let rel_paths: Vec<PathBuf> = tests
        .iter()
        .map(|p| p.strip_prefix(repo_root).unwrap_or(p).to_path_buf())
        .collect();

    // Inspect file extensions to identify test runner ecosystem
    let has_py = rel_paths
        .iter()
        .any(|p| p.extension().and_then(|e| e.to_str()) == Some("py"));
    let has_rs = rel_paths
        .iter()
        .any(|p| p.extension().and_then(|e| e.to_str()) == Some("rs"));
    let has_js = rel_paths.iter().any(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "ts" || e == "tsx" || e == "js" || e == "jsx")
            .unwrap_or(false)
    });
    let has_go = rel_paths
        .iter()
        .any(|p| p.extension().and_then(|e| e.to_str()) == Some("go"));

    if has_py {
        // Python ecosystem: construct pytest invocation targeting impacted test modules
        let paths: Vec<String> = rel_paths.iter().map(|p| p.display().to_string()).collect();
        Some(format!("pytest {}", paths.join(" ")))
    } else if has_rs {
        // Rust ecosystem: construct cargo test invocation targeting integration test binaries
        let mut test_targets = Vec::new();
        for p in &rel_paths {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                test_targets.push(format!("--test {}", stem));
            }
        }
        if !test_targets.is_empty() {
            Some(format!("cargo test {}", test_targets.join(" ")))
        } else {
            Some("cargo test".to_string())
        }
    } else if has_js {
        // JavaScript/TypeScript ecosystem: pass test paths to npm test runner
        let paths: Vec<String> = rel_paths.iter().map(|p| p.display().to_string()).collect();
        Some(format!("npm test -- {}", paths.join(" ")))
    } else if has_go {
        // Go ecosystem: execute go test on parent package directories of impacted files
        let mut dirs: Vec<String> = rel_paths
            .iter()
            .filter_map(|p| {
                p.parent().map(|d| {
                    let s = d.display().to_string();
                    if s.is_empty() {
                        ".".to_string()
                    } else {
                        format!("./{}", s)
                    }
                })
            })
            .collect();
        dirs.sort();
        dirs.dedup();
        Some(format!("go test {}", dirs.join(" ")))
    } else {
        // Generic fallback: invoke test command with relative file paths
        let paths: Vec<String> = rel_paths.iter().map(|p| p.display().to_string()).collect();
        Some(format!("test {}", paths.join(" ")))
    }
}

/// Returns list of paths that git rollback/clean operations must never delete.
///
/// Ensures both the `.agent-state` directory and the specific state file name
/// are preserved during git clean so failure records and task progress survive.
///
/// # Parameters
/// - `state_path`: The absolute or canonical path to the active state file.
///
/// # Returns
/// A `Vec<String>` containing paths/patterns to pass into git's exclude flags.
fn preserved(state_path: &Path) -> Vec<String> {
    let mut keep = vec![".agent-state".to_string()];
    if let Some(name) = state_path.file_name().and_then(|n| n.to_str()) {
        if !keep.iter().any(|p| p == name) {
            keep.push(name.to_string());
        }
    }
    keep
}

/// Resolves the repository root that owns the specified state file.
///
/// Traverses up from `state_path` looking for a `.git` directory, and falls back
/// to `fallback` if `resolve_project_root` does not find a repository marker.
///
/// # Parameters
/// - `state_path`: Path to the state file being executed against.
/// - `fallback`: Fallback repository root path if discovery fails.
///
/// # Returns
/// The resolved repository root `PathBuf`.
fn repo_root_for(state_path: &Path, fallback: &Path) -> PathBuf {
    let pr = resolve_project_root(state_path);
    if pr.join(".git").exists() {
        pr
    } else if fallback.join(".git").exists() {
        fallback.to_path_buf()
    } else {
        pr
    }
}

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
            let tools = list_all_mcp_tools();
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
