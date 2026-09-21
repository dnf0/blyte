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
    /// Command is rate-limited: wait, then run it.
    ///
    /// Denying a status poll is what Invariant IV's "mandatory cooldown debouncing" was first
    /// implemented as, and it removed the agent's only means of awaiting a background task. The
    /// denial text even instructed it to "yield turn to await background task completion
    /// notification" -- which, in headless print mode, ends the run, because nothing will ever
    /// deliver that notification. Three consecutive yields exhaust max_retries and the task is
    /// blocked: measured on vanilla (EXP-071), katex (EXP-074) and scc (EXP-076).
    ///
    /// Throttling saves the same tokens -- one poll per window rather than a loop -- without
    /// ever handing the agent an instruction to stop.
    Throttle { wait_sec: u64, reason: String },
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

        // A repeated command is NOT, by itself, a busy-wait poll.
        //
        // This used to key every identical shell command for debouncing, which meant a TDD
        // worker re-running `go test ./...` after editing code hit the cooldown -- the command
        // is identical, the repository state is not. Invariant IV blocks "runaway busy-wait
        // polling loops"; Invariant V mandates exactly this re-run loop, so debouncing it made
        // the two invariants contradict each other. The polling signatures above (procfs,
        // process inspection, manage_task status/list) are what the guard is for.
        return None;
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
            return CommandGuardDecision::Throttle {
                wait_sec: remaining,
                reason: format!(
                    "[superstate-guard] Polling throttled: this command ran {}s ago, so it was \
                     delayed {}s rather than repeated immediately. Continue working; do not end \
                     your turn to wait.",
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
    if let Some(parent) = state_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&history) {
        let parent = state_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let temp_file_path = parent.join(format!(".command_guard.{}.{}.tmp", std::process::id(), nanos));
        if fs::write(&temp_file_path, serialized.as_bytes()).is_ok() {
            let _ = fs::rename(&temp_file_path, &state_path);
        }
    }

    CommandGuardDecision::Allow
}
