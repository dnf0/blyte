//! PreToolUse file read guard CLI command handler.
//!
//! Evaluates incoming tool call payloads on `stdin` conforming to agent
//! PreToolUse protocols (e.g. Antigravity, Claude Code) against deterministic
//! context boundaries and line budgets.

use clap::{Args, Subcommand};
use std::io::Read;
use std::path::Path;
use superstate_core::guard::{evaluate_file_read_guard, FileReadGuardConfig, GuardDecision};

/// Subcommands for the `superstate guard` namespace.
#[derive(Subcommand, Debug, Clone)]
pub enum GuardCommands {
    /// Evaluate tool invocation against deterministic context boundaries
    PreToolUse(PreToolUseArgs),
}

/// Command-line arguments for `superstate guard pre-tool-use`.
#[derive(Args, Debug, Clone)]
pub struct PreToolUseArgs {
    /// Maximum lines permitted before shunting to symbol outline
    #[arg(long, default_value_t = 200)]
    pub threshold_lines: usize,
}

/// Serializes and prints the guard decision JSON to standard output.
///
/// # Parameters
/// - `decision`: The `GuardDecision` outcome to emit.
fn emit_decision(decision: &GuardDecision) {
    // Decision is serialized to JSON string conforming to hook stdout protocol.
    // Infallible serialization of GuardDecision enum.
    if let Ok(json) = serde_json::to_string(decision) {
        println!("{}", json);
    } else {
        println!(r#"{{"decision":"allow"}}"#);
    }
}

/// Helper to emit an `allow` decision when fail-open behavior is triggered.
fn emit_allow() {
    emit_decision(&GuardDecision::Allow);
}

/// Extracts tool name and arguments from a JSON container.
///
/// Supports various agent hook payload conventions including `toolName`, `tool_name`,
/// `name`, `args`, `arguments`, `parameters`, and `tool_input`.
fn extract_from_container(
    container: &serde_json::Value,
) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    // Extract tool name string from common key variants.
    let raw_name = container
        .get("name")
        .or_else(|| container.get("toolName"))
        .or_else(|| container.get("tool_name"))
        .and_then(|v| v.as_str())?;

    // Extract arguments map or parse if arguments was encoded as a JSON string.
    let raw_args = container
        .get("args")
        .or_else(|| container.get("arguments"))
        .or_else(|| container.get("parameters"))
        .or_else(|| container.get("tool_input"));

    let args_map = match raw_args {
        Some(serde_json::Value::Object(map)) => map.clone(),
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(s)
                .unwrap_or_default()
        }
        _ => serde_json::Map::new(),
    };

    Some((raw_name.to_string(), args_map))
}

/// Finds the tool name and arguments map from standard and alternate hook payload formats.
///
/// Checks both nested objects (`toolCall` or `tool_call`) and top-level fields.
fn extract_tool_invocation(
    root: &serde_json::Value,
) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    // First inspect nested tool call wrapper if present.
    if let Some(nested) = root.get("toolCall").or_else(|| root.get("tool_call")) {
        if let Some(res) = extract_from_container(nested) {
            return Some(res);
        }
    }
    // Fall back to inspecting top-level payload attributes.
    extract_from_container(root)
}

/// Extracts the target file path from common argument key variants.
///
/// Returns `None` if no path key is present or the string is empty/whitespace.
fn extract_path(args: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    const PATH_KEYS: &[&str] = &[
        "AbsolutePath",
        "path",
        "file_path",
        "filePath",
        "TargetFile",
        "target_file",
        "file",
    ];

    for &key in PATH_KEYS {
        if let Some(val) = args.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Extracts a 1-indexed line number from the arguments map.
///
/// Handles numeric integers as well as string-encoded numbers.
fn extract_line(args: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<usize> {
    for &key in keys {
        if let Some(val) = args.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(n) = val.as_i64() {
                if n >= 0 {
                    return Some(n as usize);
                }
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Executes the `pre-tool-use` hook logic.
///
/// # Parameters
/// - `args`: CLI arguments specifying threshold limits.
///
/// # Returns
/// - `Ok(())` on completion; exits 0 in all cases (fail-open on parse error).
pub fn cmd_pre_tool_use(args: PreToolUseArgs) -> Result<(), String> {
    // Read complete payload from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // Fail-open: if stdin read encounters an I/O error, allow tool invocation.
        emit_allow();
        return Ok(());
    }

    // Fail-open if stdin is empty.
    if input.trim().is_empty() {
        emit_allow();
        return Ok(());
    }

    // Parse JSON root payload; fail-open on malformed JSON to prevent agent aborts.
    let root: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            emit_allow();
            return Ok(());
        }
    };

    // Extract tool invocation; fail-open if no recognisable tool invocation is found.
    let (tool_name, tool_args) = match extract_tool_invocation(&root) {
        Some(inv) => inv,
        None => {
            emit_allow();
            return Ok(());
        }
    };

    // Normalize tool name to lowercase and verify it is a file-reading tool.
    let norm_name = tool_name.to_lowercase();
    if !matches!(
        norm_name.as_str(),
        "view_file" | "view" | "read_file" | "readfile" | "viewfile"
    ) {
        // Non-file-reading tools are permitted unconditionally.
        emit_allow();
        return Ok(());
    }

    // Extract target file path; fail-open if path is absent or blank.
    let path_str = match extract_path(&tool_args) {
        Some(p) => p,
        None => {
            emit_allow();
            return Ok(());
        }
    };

    // Extract optional line range boundaries.
    let mut start_line = extract_line(
        &tool_args,
        &["StartLine", "start_line", "startLine", "offset"],
    );
    let end_line = extract_line(&tool_args, &["EndLine", "end_line", "endLine", "limit"]);

    // Crucial Normalization: If end_line is specified without start_line (e.g. head slice),
    // default start_line to 1 so the bounded slice calculation succeeds.
    if end_line.is_some() && start_line.is_none() {
        start_line = Some(1);
    }

    // Configure guard with CLI threshold settings.
    let config = FileReadGuardConfig {
        threshold_lines: args.threshold_lines,
    };

    // Evaluate proposed file read via superstate-core engine.
    let decision = evaluate_file_read_guard(Path::new(&path_str), start_line, end_line, &config);
    emit_decision(&decision);

    Ok(())
}

/// Dispatches guard subcommands to their respective handlers.
pub fn dispatch_guard(cmd: GuardCommands) -> Result<(), String> {
    match cmd {
        GuardCommands::PreToolUse(args) => cmd_pre_tool_use(args),
    }
}
