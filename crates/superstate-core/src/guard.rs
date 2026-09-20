//! PreToolUse file read guard engine and AST shunting logic.
//!
//! This module intercepts file read attempts before their payload enters an agent's
//! LLM context window. When an agent requests a read exceeding the threshold budget
//! without explicit range bounds, the guard blocks the read and returns an AST symbol
//! outline or structural preview, guiding the agent to request bounded slices.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use serde::{Deserialize, Serialize};

/// Decision returned by the file read guard evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum GuardDecision {
    /// Read is permitted to proceed into agent context.
    Allow,
    /// Read is blocked; `reason` contains guidance and AST outline or line preview.
    Deny { reason: String },
}

/// Configuration settings for the file read guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadGuardConfig {
    /// Maximum line count permitted for an unbounded or bounded file read.
    pub threshold_lines: usize,
}

impl Default for FileReadGuardConfig {
    /// Default threshold is 200 lines, bounding token ingestion per tool call.
    fn default() -> Self {
        Self {
            threshold_lines: 200,
        }
    }
}

/// Evaluates whether a proposed file read is permitted or should be shunted.
///
/// # Parameters
/// - `path`: Path to the target file to be read.
/// - `start_line`: Optional 1-indexed starting line number of the requested slice.
/// - `end_line`: Optional 1-indexed ending line number of the requested slice.
/// - `config`: File read guard configuration holding line thresholds.
///
/// # Returns
/// - `GuardDecision::Allow` if the file does not exist, is a directory, is within
///   budget, or has a bounded slice within threshold.
/// - `GuardDecision::Deny` if the file is binary or exceeds the line budget.
pub fn evaluate_file_read_guard(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
    config: &FileReadGuardConfig,
) -> GuardDecision {
    // Non-existent target: allow tool invocation so standard runtime errors surface to agent.
    if !path.exists() {
        return GuardDecision::Allow;
    }

    // Directory target: allow tool invocation so standard directory reading error handling runs.
    if path.is_dir() {
        return GuardDecision::Allow;
    }

    // Attempt to open file to inspect the initial byte header for binary detection.
    // If opening fails (e.g. permission error), allow tool execution so runtime reports error.
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return GuardDecision::Allow,
    };

    // Read up to 8KB to inspect for null bytes indicating non-text binary formats.
    let mut buf = [0u8; 8192];
    let bytes_read = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return GuardDecision::Allow,
    };

    // Block binary files from entering LLM context to avoid corrupting agent prompt state.
    if buf[..bytes_read].contains(&0x00) {
        return GuardDecision::Deny {
            reason: format!(
                "[superstate-guard] Binary file read blocked: {}",
                path.display()
            ),
        };
    }

    // Read full text content to count lines and extract AST symbol outlines if needed.
    // If read_to_string fails (e.g. non-UTF8), allow runtime tool to handle error.
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return GuardDecision::Allow,
    };

    // Calculate total line count in file.
    let total_lines = content.lines().count();

    // Files within the line budget are permitted unconditionally.
    if total_lines <= config.threshold_lines {
        return GuardDecision::Allow;
    }

    // If caller specified an explicit slice and the span is within threshold, permit read.
    if let (Some(start), Some(end)) = (start_line, end_line) {
        if end.saturating_sub(start) <= config.threshold_lines {
            return GuardDecision::Allow;
        }
    }

    // Attempt AST extraction via superstate-syntax for supported languages (Rust, Python, TS, Go).
    let ast_outline = superstate_syntax::inspect::detect_language(path)
        .and_then(|lang| superstate_syntax::symbols::extract_symbols(&content, lang).ok())
        .map(|symbols| superstate_syntax::inspect::format_outline(&symbols))
        .filter(|outline| !outline.trim().is_empty());

    // When an AST outline is available, return symbol structure so agent can target specific symbols.
    if let Some(outline) = ast_outline {
        GuardDecision::Deny {
            reason: format!(
                "[superstate-guard] Full file read blocked ({} lines exceeds {}-line budget).\n\nAST Symbol Outline:\n{}\n\nRe-read using bounded StartLine and EndLine (<= {} lines), or inspect specific symbols with 'superstate inspect'.",
                total_lines, config.threshold_lines, outline, config.threshold_lines
            ),
        }
    } else {
        // Fall back to a structural preview of first 30 lines for unsupported languages or symbol-less files.
        let preview = content
            .lines()
            .take(30)
            .collect::<Vec<_>>()
            .join("\n");
        GuardDecision::Deny {
            reason: format!(
                "[superstate-guard] Full file read blocked ({} lines exceeds {}-line budget).\n\nFile preview (first 30 lines):\n{}\n...\nRe-read using bounded StartLine and EndLine (<= {} lines).",
                total_lines, config.threshold_lines, preview, config.threshold_lines
            ),
        }
    }
}
