//! PreToolUse command and polling guard engine.
//!
//! Intercepts process inspection, task status checks, and repeated commands before
//! their execution. By enforcing a deterministic cooldown debounce, this engine blocks
//! wasteful LLM busy-wait polling loops while tasks run in the background.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Compiled regular expression matching procfs process interrogation patterns.
///
/// Detects attempts to poll `/proc/<pid>/cmdline`, `/proc/<pid>/stat`, or `/proc/<pid>/status`.
static PROCFS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/proc/(\d+|\*)/(cmdline|stat|status)").expect("valid procfs regex")
});

/// Compiled regular expression matching process interrogation and inspection commands.
///
/// Detects invocations of process list inspection (`ps aux`, `ps -ef`, `pgrep`, `top`, `htop`, `jobs`).
static PROCESS_INSPECT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(ps\s+aux|ps\s+-ef|pgrep|top|htop|jobs)\b")
        .expect("valid process inspection regex")
});

/// Configuration parameters for command cooldown and polling debounce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandGuardConfig {
    /// Number of seconds a polling classification remains on cooldown before next permitted run.
    pub debounce_sec: u64,
    /// Optional custom path for the state history JSON document.
    pub state_path: Option<PathBuf>,
}

impl Default for CommandGuardConfig {
    /// Defaults to a 30-second cooldown window and default workspace state path.
    fn default() -> Self {
        Self {
            debounce_sec: 30,
            state_path: None,
        }
    }
}

/// Outcome of evaluating a proposed tool execution against the command guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CommandGuardDecision {
    /// Command is permitted to execute.
    Allow,
    /// Command is blocked by an active polling cooldown.
    Deny { reason: String },
}

/// Normalizes command line string by collapsing contiguous whitespace tokens.
///
/// Ensures minor whitespace variations (e.g. extra spaces) do not bypass duplicate detection.
fn normalize_command(cmd: &str) -> String {
    cmd.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Determines whether a tool name corresponds to a shell execution tool.
fn is_shell_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "run_command"
            | "runcommand"
            | "bash"
            | "sh"
            | "execute_command"
            | "executecommand"
            | "shell"
            | "exec_command"
            | "run_shell_command"
            | "terminal"
    )
}

/// Determines if a tool invocation represents a polling command and derives its state tracking key.
///
/// # Parameters
/// - `tool_name`: Identifier of the tool being called (e.g. `run_command`, `manage_task`).
/// - `args`: JSON arguments map passed to the tool.
///
/// # Returns
/// - `Some(String)` containing the classification key if the command is subject to cooldown.
/// - `None` if the command is non-polling or an unrelated tool.
fn classify_tool_invocation(
    tool_name: &str,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    // Inspect manage_task tool calls for polling actions
    if tool_name.eq_ignore_ascii_case("manage_task") {
        let action = args
            .get("Action")
            .or_else(|| args.get("action"))
            .and_then(|v| v.as_str())?;

        let norm_action = action.trim().to_ascii_lowercase();
        // Only "status" and "list" trigger the polling cooldown guard; "kill" and "send_input" pass through
        if norm_action == "status" || norm_action == "list" {
            return Some("manage_task_status".to_string());
        }
        return None;
    }

    // Inspect shell execution tools for procfs, process inspection, or exact duplicates
    if is_shell_tool(tool_name) {
        let cmd = args
            .get("CommandLine")
            .or_else(|| args.get("command_line"))
            .or_else(|| args.get("commandLine"))
            .or_else(|| args.get("command"))
            .or_else(|| args.get("cmd"))
            .or_else(|| args.get("script"))
            .and_then(|v| v.as_str())?;

        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Procfs polling patterns (e.g. /proc/123/status)
        if PROCFS_RE.is_match(trimmed) {
            return Some("procfs_polling".to_string());
        }

        // Process inspection patterns (e.g. ps aux, pgrep)
        if PROCESS_INSPECT_RE.is_match(trimmed) {
            return Some("process_inspection".to_string());
        }

        // Exact duplicate command detection keyed by normalized command string
        let normalized = normalize_command(trimmed);
        return Some(format!("cmd:{}", normalized));
    }

    // Unrelated tools are never debounced
    None
}

/// Evaluates whether a proposed tool execution is permitted or denied by an active cooldown.
///
/// Adheres strictly to fail-open behavior: any file I/O error, corrupt state file, or permission
/// issue results in permitting the command rather than failing tool execution.
///
/// # Parameters
/// - `tool_name`: Identifier of the tool being called.
/// - `args`: JSON argument map supplied with the tool invocation.
/// - `config`: Guard configuration containing debounce interval and state path.
///
/// # Returns
/// - `CommandGuardDecision::Allow` if allowed or fail-open triggered.
/// - `CommandGuardDecision::Deny` if cooldown is currently active.
pub fn evaluate_command_guard(
    tool_name: &str,
    args: &serde_json::Map<String, serde_json::Value>,
    config: &CommandGuardConfig,
) -> CommandGuardDecision {
    // 1. Tool Classification: return Allow immediately if not a debounced polling pattern
    let classification_key = match classify_tool_invocation(tool_name, args) {
        Some(k) => k,
        None => return CommandGuardDecision::Allow,
    };

    // 2. Resolve state file location (defaults to .agent-state/command_guard_history.json)
    let state_path = config
        .state_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(".agent-state/command_guard_history.json"));

    // 3. Current timestamp in seconds since UNIX epoch
    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // 4. Load history map from disk (fail-open on corrupt JSON or missing file)
    let mut history: BTreeMap<String, u64> = if state_path.exists() {
        fs::read_to_string(&state_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };

    // 5. Cooldown check: if entry exists and within debounce window, deny execution
    if let Some(&last_sec) = history.get(&classification_key) {
        let elapsed = now_sec.saturating_sub(last_sec);
        if elapsed < config.debounce_sec {
            let remaining = config.debounce_sec.saturating_sub(elapsed);
            return CommandGuardDecision::Deny {
                reason: format!(
                    "[superstate-guard] Busy-wait polling blocked: command was executed {}s ago. Cooldown active ({}s remaining). Do not loop on status; yield turn to await background task completion notification.",
                    elapsed, remaining
                ),
            };
        }
    }

    // 6. Prune stale history entries older than 2x debounce window to bound file size
    let retention_limit = config.debounce_sec.saturating_mul(2);
    history.retain(|_, &mut ts| now_sec.saturating_sub(ts) <= retention_limit);

    // 7. Update timestamp for the current classification key
    history.insert(classification_key, now_sec);

    // 8. Persist updated history atomically to disk; ignore write errors under fail-open policy
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&history) {
        let parent = state_path.parent().unwrap_or_else(|| Path::new("."));
        let temp_file_path = parent.join(format!(".command_guard.{}.tmp", std::process::id()));
        if fs::write(&temp_file_path, serialized.as_bytes()).is_ok() {
            let _ = fs::rename(&temp_file_path, &state_path);
        }
    }

    CommandGuardDecision::Allow
}
