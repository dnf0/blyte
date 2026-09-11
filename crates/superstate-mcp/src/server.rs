//! Native Model Context Protocol (MCP) server for Superstate.
//!
//! Provides JSON-RPC 2.0 stdio transport handling tool listing and execution
//! for agentic workflows (Claude Code, Antigravity, Cursor).

use crate::tools::list_all_mcp_tools;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use superstate_core::ledger::append_ledger_note;
use superstate_core::models::{LedgerEntry, MilestoneSpec};
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::prompt::render_turnkey_prompt;
use superstate_core::state_machine::{
    advance_step, init_state_direct, init_state_milestones, load_state, save_state_atomic,
};
use superstate_core::verifier::execute_all_gates;
use superstate_worktree::manager::{
    ensure_git_baseline, ensure_git_exclude, IsolatedWorktree, WorktreeManager,
};
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

            // If verify_cmds array is provided, iterate and dry-run each command
            if let Some(cmds_val) = args.get("verify_cmds") {
                let cmds: Vec<String> = cmds_val
                    .as_array()
                    .ok_or_else(|| "'verify_cmds' must be an array of strings".to_string())?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(ToString::to_string)
                            .ok_or_else(|| "Each entry in 'verify_cmds' must be a string".to_string())
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

            let state_path = args
                .get("state")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root.join(".agent-state").join("state.json"));

            let no_worktree = args
                .get("no_worktree")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

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
                let mut st = init_state_milestones(
                    goal,
                    milestones,
                    base_branch.as_deref(),
                    no_worktree,
                )
                .map_err(|e| format!("Failed to initialize state from milestones: {}", e))?;

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
                    "Must provide either 'milestones' array or both 'goal' and 'verify_cmd'".to_string(),
                );
            };

            // Atomically persist state to disk
            save_state_atomic(&state_path, &state)
                .map_err(|e| format!("Failed to save state: {}", e))?;

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
            let plan = args
                .get("plan")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'plan'".to_string())?;

            let repo_root = args
                .get("repo_root")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path = args
                .get("state")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root.join(".agent-state").join("state.json"));

            ensure_git_baseline(&repo_root);
            let plan_path = PathBuf::from(plan);
            let state = init_state_from_plan(&plan_path, &state_path)
                .map_err(|e| format!("Plan parse error: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "step": state.step,
                "active_task": state.active_task
            }))
        }
        "state_next" => {
            let verify_cmd = args
                .get("verify_cmd")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let verify_git_clean = args
                .get("verify_git_clean")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let repo_root = args
                .get("repo_root")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path = args
                .get("state")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root.join(".agent-state").join("state.json"));

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

            let results = execute_all_gates(&target_dir, &task_to_verify, verify_git_clean)
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
                        format!(
                            "wt/{}",
                            superstate_worktree::manager::sanitize_slug(&active_task.id)
                        )
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
                "active_task": state.active_task,
                "active_worktree": state.active_worktree.as_ref().map(|p| p.display().to_string()),
                "active_branch": state.active_branch,
            });

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
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path = args
                .get("state")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root.join(".agent-state").join("state.json"));

            let state = load_state(&state_path).map_err(|e| format!("Load state error: {}", e))?;
            let task = args.get("task").and_then(|v| v.as_str());
            let prompt =
                render_turnkey_prompt(&state, &state_path, task, state.no_worktree, false)?;

            Ok(serde_json::json!({
                "turnkey_prompt": prompt,
                "content": [{ "type": "text", "text": prompt }]
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
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let state_path = args
                .get("state")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root.join(".agent-state").join("state.json"));

            append_ledger_note(&state_path, LedgerEntry::new(author, note))
                .map_err(|e| format!("Note error: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "note_recorded": true
            }))
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
        unknown => Err(format!("Unknown tool: {}", unknown)),
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
            let resp = handle_json_rpc(req);
            let resp_str = serde_json::to_string(&resp)?;
            writeln!(stdout, "{}", resp_str)?;
            stdout.flush()?;
        }
    }

    Ok(())
}
