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
use superstate_core::models::LedgerEntry;
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::prompt::render_turnkey_prompt;
use superstate_core::state_machine::{
    advance_step, init_state_direct, load_state, save_state_atomic,
};
use superstate_worktree::manager::WorktreeManager;
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
pub fn execute_preflight(verify_cmd: &str, cwd: &Path) -> PreflightResult {
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("sh");
    #[cfg(not(windows))]
    cmd.arg("-c").arg(verify_cmd);

    #[cfg(windows)]
    let mut cmd = std::process::Command::new("cmd");
    #[cfg(windows)]
    cmd.args(["/C", verify_cmd]);

    cmd.current_dir(cwd);

    match cmd.output() {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let executable = output.status.success();
            let stdout_raw = String::from_utf8_lossy(&output.stdout);
            let stderr_raw = String::from_utf8_lossy(&output.stderr);
            PreflightResult {
                executable,
                exit_code,
                stdout: clip_string(&stdout_raw, 500),
                stderr: clip_string(&stderr_raw, 500),
            }
        }
        Err(e) => PreflightResult {
            executable: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: clip_string(&e.to_string(), 500),
        },
    }
}

/// Ensure essential Superstate internal paths are added to .git/info/exclude.
fn ensure_git_exclude(repo_root: &Path) {
    let exclude_path = repo_root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let entries = [
        ".agent-state/",
        ".wt/",
        ".wt",
        "plan.md",
        "plan.normalized.md",
        "run-in-container",
    ];
    if let Ok(mut current) = std::fs::read_to_string(&exclude_path) {
        let mut appended = false;
        for entry in &entries {
            if !current.lines().any(|l| l.trim() == *entry) {
                if !current.ends_with('\n') && !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(entry);
                current.push('\n');
                appended = true;
            }
        }
        if appended {
            let _ = std::fs::write(&exclude_path, current);
        }
    } else {
        let initial = ".agent-state/\n.wt/\n.wt\nplan.md\nplan.normalized.md\nrun-in-container\n";
        let _ = std::fs::write(&exclude_path, initial);
    }
}

/// Ensure a baseline git repository exists for worktree and change tracking.
fn ensure_git_baseline(repo_root: &Path) {
    let check = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(repo_root)
        .output();

    let inside_git = match check {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    };

    if !inside_git {
        let init_status = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo_root)
            .status();

        let ok = match init_status {
            Ok(s) if s.success() => true,
            _ => std::process::Command::new("git")
                .args(["init"])
                .current_dir(repo_root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
        };

        if ok {
            let _ = std::process::Command::new("git")
                .args(["config", "user.name", "Superstate"])
                .current_dir(repo_root)
                .status();
            let _ = std::process::Command::new("git")
                .args(["config", "user.email", "superstate@local"])
                .current_dir(repo_root)
                .status();
            let _ = std::process::Command::new("git")
                .args([
                    "commit",
                    "--allow-empty",
                    "--no-gpg-sign",
                    "-m",
                    "chore(superstate): initialize tracking baseline",
                ])
                .current_dir(repo_root)
                .status();
        }
    }
    ensure_git_exclude(repo_root);
}

/// Dispatch a tool execution by name and arguments.
///
/// # Arguments
/// * `name` - The tool name requested by the JSON-RPC caller.
/// * `args` - The arguments JSON object.
///
/// # Errors
/// Returns an error message string if required arguments are missing or tool execution fails.
fn handle_tool_call(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "superstate_preflight" => {
            let verify_cmd = args
                .get("verify_cmd")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'verify_cmd'".to_string())?;

            let cwd = args
                .get("cwd")
                .or_else(|| args.get("repo_root"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let res = execute_preflight(verify_cmd, &cwd);

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
            let goal = args
                .get("goal")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'goal'".to_string())?;

            let verify_cmd = args
                .get("verify_cmd")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'verify_cmd'".to_string())?;

            let no_worktree = args
                .get("no_worktree")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

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

            // Ensure baseline repository setup and git exclude invariants
            ensure_git_baseline(&repo_root);

            // Execute dry-run validation of the verify_cmd
            let _preflight = execute_preflight(verify_cmd, &repo_root);

            // Directly construct ExecutionState from goal and invariant without plan.md
            let mut state = init_state_direct(goal, verify_cmd, files_modify, no_worktree);

            // Provision isolated git worktree if isolation is enabled and git repository exists
            if !no_worktree && repo_root.join(".git").exists() {
                ensure_git_exclude(&repo_root);
                if let Ok(manager) = WorktreeManager::new(&repo_root) {
                    if let Some(ref active) = state.active_task {
                        if let Ok(wt) = manager.ensure_task_worktree(&active.id, None) {
                            state.active_worktree = Some(wt.path.clone());
                            state.active_branch = Some(wt.branch.clone());
                        }
                    }
                }
            }

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

            let mut state =
                load_state(&state_path).map_err(|e| format!("Load state error: {}", e))?;
            advance_step(&mut state).map_err(|e| format!("Advance step error: {}", e))?;
            save_state_atomic(&state_path, &state)
                .map_err(|e| format!("Save state error: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "step": state.step,
                "active_task": state.active_task
            }))
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
