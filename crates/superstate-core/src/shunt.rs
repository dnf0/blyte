//! Output distillation and log storage engine ("Spotify Portal" pattern).
//!
//! Intercepts process output and determines whether it should pass through verbatim
//! (if under configured line and byte limits) or be distilled into a compact summary
//! while persisting the full raw output to disk in `.agent-state/logs/`.
//! This prevents LLM context exhaustion on massive command outputs (builds, test suites).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Configuration thresholds and paths for tool output shunting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShuntConfig {
    /// Maximum number of combined output lines before shunting triggers.
    pub threshold_lines: usize,
    /// Maximum number of combined output bytes before shunting triggers.
    pub threshold_bytes: usize,
    /// Directory where full unstripped command logs are stored.
    pub logs_dir: PathBuf,
}

impl Default for ShuntConfig {
    fn default() -> Self {
        Self {
            threshold_lines: 50,
            threshold_bytes: 4096,
            logs_dir: PathBuf::from(".agent-state/logs"),
        }
    }
}

/// The evaluated outcome of process output through the shunt layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistilledOutput {
    /// Output was small enough to pass through unaltered with zero latency overhead.
    Raw { stdout: Vec<u8>, stderr: Vec<u8> },
    /// Output exceeded threshold limits and was summarized with a pointer to the log file.
    Distilled {
        summary: String,
        log_path: PathBuf,
        exit_code: i32,
    },
}

/// Evaluates process output against thresholds and returns either raw or distilled output.
///
/// If combined line count and byte size fall within configured thresholds, returns
/// [`DistilledOutput::Raw`]. Otherwise, persists the full raw stdout and stderr to a unique log file
/// inside the logs directory and formats a concise high-signal summary as [`DistilledOutput::Distilled`].
///
/// # Parameters
/// - `cmd_str`: The command string executed (recorded in the log header and hashed for filename).
/// - `working_dir`: The directory in which the command executed.
/// - `stdout`: Raw standard output bytes captured from the child process.
/// - `stderr`: Raw standard error bytes captured from the child process.
/// - `exit_code`: The integer exit status of the child process.
/// - `config`: Shunting configuration containing thresholds and log directory.
///
/// # Returns
/// - `Ok(DistilledOutput)` on successful evaluation and log persistence.
/// - `Err(std::io::Error)` if log directory creation or file writing fails.
pub fn evaluate_and_distill(
    cmd_str: &str,
    working_dir: &Path,
    stdout: &[u8],
    stderr: &[u8],
    exit_code: i32,
    config: &ShuntConfig,
) -> Result<DistilledOutput, std::io::Error> {
    // Measure combined size across streams to check byte threshold
    let combined_bytes_len = stdout.len() + stderr.len();
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    // Count lines across stdout and stderr to check line count threshold
    let total_lines = stdout_str.lines().count() + stderr_str.lines().count();

    // Guard clause: small output passes through verbatim to avoid unnecessary disk I/O and latency
    if total_lines <= config.threshold_lines && combined_bytes_len <= config.threshold_bytes {
        return Ok(DistilledOutput::Raw {
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        });
    }

    // Resolve destination log directory relative to working_dir if relative
    let logs_dir = if config.logs_dir.is_relative() {
        working_dir.join(&config.logs_dir)
    } else {
        config.logs_dir.clone()
    };
    fs::create_dir_all(&logs_dir)?;

    // Protect logs directory with a local .gitignore so raw logs are never committed
    let gitignore_path = logs_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let _ = fs::write(&gitignore_path, "*\n!.gitignore\n");
    }

    // Derive deterministic unique log file path with millisecond timestamp and command hash
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let hash = format!("{:016x}", fnv1a_hash(cmd_str.as_bytes()));
    let log_filename = format!("cmd_{}_{}.log", stamp, &hash[..8]);
    let log_path = logs_dir.join(log_filename);

    // Write full execution context and raw uncorrupted streams to disk
    let mut file = fs::File::create(&log_path)?;
    writeln!(file, "=== Command: {} ===", cmd_str)?;
    writeln!(file, "=== Exit Code: {} ===", exit_code)?;
    writeln!(file, "=== Working Dir: {} ===", working_dir.display())?;
    writeln!(file, "=== Timestamp: {} ===\n", stamp)?;
    if !stdout.is_empty() {
        writeln!(file, "--- stdout ---")?;
        file.write_all(stdout)?;
        writeln!(file)?;
    }
    if !stderr.is_empty() {
        writeln!(file, "--- stderr ---")?;
        file.write_all(stderr)?;
        writeln!(file)?;
    }

    // Format distillation according to process outcome
    let summary = if exit_code == 0 {
        format!(
            "[superstate-shunt] Command succeeded (exit 0). {} lines omitted.\nFull log: {}",
            total_lines,
            log_path.display()
        )
    } else {
        let failure_excerpt = extract_failure_excerpt(&stdout_str, &stderr_str);
        format!(
            "[superstate-shunt] Command failed with exit code {} ({} lines omitted).\nFull log: {}\n\n--- Failure Excerpt ---\n{}\n--- End Failure Excerpt ---",
            exit_code,
            total_lines,
            log_path.display(),
            failure_excerpt
        )
    };

    Ok(DistilledOutput::Distilled {
        summary,
        log_path,
        exit_code,
    })
}

/// Computes a fast deterministic 64-bit FNV-1a hash of the input bytes.
///
/// Anchored on standard 64-bit FNV offset basis and prime for lightweight hashing
/// without introducing additional cryptographic crate dependencies.
///
/// # Parameters
/// - `bytes`: Raw byte slice to hash.
///
/// # Returns
/// - 64-bit integer hash value.
fn fnv1a_hash(bytes: &[u8]) -> u64 {
    // Standard 64-bit FNV-1a offset basis and prime constants
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Known failure signatures in stdout/stderr from common test runners and compilers.
const FAILURE_PATTERNS: &[&str] = &[
    "FAIL:",
    "--- FAIL:",
    "FAILED",
    "Error:",
    "AssertionError",
    "panic:",
    "undefined:",
    "TypeError:",
    "Exception:",
    "error[E",
];

/// Extracts the most salient error lines and trailing summary from failed command output.
///
/// Searches for known failure tokens across stdout and stderr. If matches are found, includes
/// context surrounding the match (2 lines prior, 3 lines after) up to a limit of 15 lines,
/// along with the last 5 trailing summary lines. If no patterns match, falls back to the last
/// 15 lines of output.
///
/// # Parameters
/// - `stdout`: Process standard output as string slice.
/// - `stderr`: Process standard error as string slice.
///
/// # Returns
/// - Extracted failure snippet string.
pub fn extract_failure_excerpt(stdout: &str, stderr: &str) -> String {
    // Combine stdout and stderr into unified output stream for excerpt extraction
    let combined = match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{}\n{}", stdout, stderr),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (true, true) => String::new(),
    };
    let lines: Vec<&str> = combined.lines().collect();
    let mut excerpt_lines: Vec<&str> = Vec::new();

    // Scan for error and assertion failure tokens to isolate critical failure context
    for (i, line) in lines.iter().enumerate() {
        if FAILURE_PATTERNS.iter().any(|&pat| line.contains(pat)) {
            let start = i.saturating_sub(2);
            let end = (i + 4).min(lines.len());
            for &excerpt_line in &lines[start..end] {
                if !excerpt_lines.contains(&excerpt_line) {
                    excerpt_lines.push(excerpt_line);
                }
            }
            if excerpt_lines.len() >= 15 {
                break;
            }
        }
    }

    // Fall back to trailing lines when output lacks recognised failure tokens
    if excerpt_lines.is_empty() {
        let start = lines.len().saturating_sub(15);
        excerpt_lines.extend_from_slice(&lines[start..]);
    } else {
        // Append trailing summary stats (e.g. "Tests: 1 failed, 79 passed") if not already present
        let tail_start = lines.len().saturating_sub(5);
        for &line in &lines[tail_start..] {
            if !excerpt_lines.contains(&line) {
                excerpt_lines.push(line);
            }
        }
    }

    excerpt_lines.join("\n")
}
