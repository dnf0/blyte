//! PreToolUse file read guard engine and AST shunting logic.
//!
//! This module intercepts file read attempts before their payload enters an agent's
//! LLM context window. When an agent requests a read exceeding the threshold budget
//! without explicit range bounds, the guard blocks the read and returns an AST symbol
//! outline or structural preview, guiding the agent to request bounded slices.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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
        let preview = content.lines().take(30).collect::<Vec<_>>().join("\n");
        GuardDecision::Deny {
            reason: format!(
                "[superstate-guard] Full file read blocked ({} lines exceeds {}-line budget).\n\nFile preview (first 30 lines):\n{}\n...\nRe-read using bounded StartLine and EndLine (<= {} lines).",
                total_lines, config.threshold_lines, preview, config.threshold_lines
            ),
        }
    }
}

/// Default line budget before a read is served as an AST outline instead.
///
/// Chosen from measurement, and corrected by it. A tool call carries a fixed preamble of
/// roughly 2,500-5,000 tokens (FINDINGS.md 13.17), so splitting a file pays that cost once per
/// window to avoid fetching the lines it skips. Too low and the preamble dominates; too high
/// and whole files arrive that nobody wanted.
///
/// This value has been wrong twice, in both directions, and the history is the point:
///
/// * 200 was set by intuition. Replaying a real trace, a 1,906-line file took 14 calls costing
///   50,719 tokens where one whole-file read would have cost 23,466 -- 2.2x worse (13.18).
/// * 1,600 replaced it briefly on that same replay, then 800 as a hedge. Both came from ONE
///   trace of `superstate-core`, whose only file above 1,600 lines is `daemon.rs` itself, so
///   that "optimum" marked where the budget stops binding rather than a cost minimum (13.20).
/// * 400 comes from two independent real-task traces -- a Go repository and a TypeScript one --
///   replayed at 17 thresholds. Both minimise at exactly 400, inside a shallow basin of roughly
///   300-450. At 800 katex cost 151,562 against 133,861, which is worse than the 200 it had
///   replaced (13.21).
///
/// Treat the basin, not the integer, as the finding: anything in 300-450 is defensible. The
/// cost model assumes wanted lines repack optimally into windows, which flatters HIGH
/// thresholds, so the true optimum sits at or below 400 rather than above it.
///
/// Keep this in proportion. Tuning this number is worth 9-17% of read cost; serving a read
/// whose window is already held, without a tool call at all, is worth 45-68% (13.21). The
/// budget is second-order and should not be mistaken for the main mechanism.
///
/// The optimum is workload-specific -- it tracks how many lines a task actually needs from a
/// file -- so this is a default, not a constant of nature. `scripts/read_budget.py` in
/// superstate-eval recomputes it from any recorded trace.
pub const DEFAULT_READ_THRESHOLD_LINES: usize = 400;

/// Detects a shell command that dumps a whole file into the conversation.
///
/// The file read guard only ever matched the agent's *file viewing tool*, so `cat internal/x.go`
/// walked straight past it -- and past the command guard, which classifies only polling, and past
/// output shunting, which wraps `./run-in-container` and not host commands. Across recorded runs
/// 29 of 152 logged commands were `cat` or `grep`, so this is not a corner case; it is a hole in
/// the middle of the perimeter. Spotify's Portal covers the same gap with its `check-bash-read`
/// hook, alongside the file-size hook we already had.
///
/// Returns the path being dumped, or None when the command is not a bulk read. Bounded reads are
/// deliberately not matched: `head -50 x.go` and `sed -n '10,60p' x.go` are the behaviour the
/// guard is trying to encourage, so intercepting them would punish compliance.
pub fn bulk_read_target(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Consider each pipeline stage: `cat big.go | grep foo` dumps the file before grep sees it.
    for stage in trimmed.split('|') {
        let mut tokens = stage.split_whitespace();
        let Some(program) = tokens.next() else {
            continue;
        };
        let program = program.rsplit('/').next().unwrap_or(program);
        if !matches!(program, "cat" | "less" | "more" | "head" | "tail") {
            continue;
        }

        let mut operands: Vec<&str> = Vec::new();
        let mut bounded = false;
        let mut skip_next = false;
        for token in tokens {
            if skip_next {
                skip_next = false;
                continue;
            }
            if token == "-n" || token == "-c" {
                // `head -n 50` bounds the read; the count is the next token.
                bounded = true;
                skip_next = true;
                continue;
            }
            if token.starts_with('-') {
                // `head -50` and any other flag.
                if token.len() > 1 && token[1..].chars().all(|c| c.is_ascii_digit()) {
                    bounded = true;
                }
                continue;
            }
            operands.push(token);
        }

        if bounded {
            continue;
        }
        if let Some(first) = operands.first() {
            return Some(PathBuf::from(first.trim_matches(['"', '\''])));
        }
    }
    None
}
