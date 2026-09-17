use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use thiserror::Error;

use crate::diff::DiffError;
use crate::prompt::build_review_system_prompt;
use crate::schema::{ReviewFinding, ReviewReport, Severity};

#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub driver: Option<String>,
    pub timeout_secs: u64,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            driver: None,
            timeout_secs: 120,
        }
    }
}

#[derive(Error, Debug)]
pub enum ReviewError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Diff error: {0}")]
    Diff(#[from] DiffError),
    #[error("Driver execution error: {0}")]
    Driver(String),
    #[error("Driver not found in PATH or environment")]
    DriverNotFound,
    #[error("JSON parse error: {0}")]
    Json(String),
}

/// Expands driver commands for Claude and AGY to ensure headless non-interactive and permission flags.
pub fn expand_driver_command(driver: &str) -> String {
    let tokens: Vec<&str> = driver.split_whitespace().collect();
    let first_token = match tokens.first() {
        Some(&t) => t,
        None => return driver.to_string(),
    };

    let binary_name = Path::new(first_token)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(first_token);
    let is_claude = binary_name == "claude";
    let is_agy = binary_name == "agy";

    if !is_claude && !is_agy {
        return driver.to_string();
    }

    let has_print = tokens.iter().any(|&t| t == "-p" || t == "--print");

    let has_perms = if is_claude {
        tokens.iter().any(|&t| {
            t == "--dangerously-skip-permissions"
                || t == "--permission-mode=bypassPermissions"
                || t.strip_prefix("--permission-mode=")
                    .map(|val| val.trim_matches('\'').trim_matches('"') == "bypassPermissions")
                    .unwrap_or(false)
        }) || tokens.windows(2).any(|w| {
            w[0] == "--permission-mode"
                && w[1].trim_matches('\'').trim_matches('"') == "bypassPermissions"
        })
    } else {
        tokens.contains(&"--dangerously-skip-permissions")
    };

    if has_print && has_perms {
        return driver.to_string();
    }

    let mut expanded = driver.trim_end().to_string();
    if !has_print {
        expanded.push_str(" -p");
    }
    if !has_perms {
        expanded.push_str(" --dangerously-skip-permissions");
    }

    expanded
}

/// Helper to check if a binary exists and is executable in PATH.
fn is_binary_in_path(binary: &str) -> bool {
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(binary);
            if candidate.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = candidate.metadata() {
                        if metadata.permissions().mode() & 0o111 != 0 {
                            return true;
                        }
                    }
                }
                #[cfg(not(unix))]
                return true;
            }
        }
    }
    false
}

/// Resolve the driver command string based on configured driver, env vars, and PATH discovery.
pub fn resolve_review_driver(configured_driver: Option<&str>) -> Result<String, ReviewError> {
    resolve_review_driver_with_env(
        configured_driver,
        |k| std::env::var(k).ok(),
        is_binary_in_path,
    )
}

/// Internal resolution helper allowing deterministic testing of environment and PATH lookup.
pub fn resolve_review_driver_with_env<E, P>(
    configured_driver: Option<&str>,
    get_env: E,
    check_path: P,
) -> Result<String, ReviewError>
where
    E: Fn(&str) -> Option<String>,
    P: Fn(&str) -> bool,
{
    if let Some(driver) = configured_driver {
        let trimmed = driver.trim();
        if !trimmed.is_empty() {
            return Ok(expand_driver_command(trimmed));
        }
    }

    if let Some(review_driver) = get_env("SUPERSTATE_REVIEW_DRIVER") {
        let trimmed = review_driver.trim();
        if !trimmed.is_empty() {
            return Ok(expand_driver_command(trimmed));
        }
    }

    if let Some(superstate_driver) = get_env("SUPERSTATE_DRIVER") {
        let trimmed = superstate_driver.trim();
        if !trimmed.is_empty() {
            return Ok(expand_driver_command(trimmed));
        }
    }

    if check_path("agy") {
        return Ok(
            "agy --model gemini-3.8-flash-low --print --dangerously-skip-permissions".to_string(),
        );
    }

    if check_path("claude") {
        return Ok("claude -p --dangerously-skip-permissions".to_string());
    }

    Err(ReviewError::DriverNotFound)
}

/// Clean markdown code fences (e.g. ```json ... ``` or ``` ... ```) and trim whitespace.
pub fn clean_json_response(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(start_idx) = trimmed.find("```") {
        let after_fence = &trimmed[start_idx + 3..];
        let content_start = if let Some(newline_idx) = after_fence.find('\n') {
            start_idx + 3 + newline_idx + 1
        } else {
            let mut offset = start_idx + 3;
            if after_fence.to_lowercase().starts_with("json") {
                offset += 4;
            }
            offset
        };
        if let Some(end_idx) = trimmed[content_start..].rfind("```") {
            return trimmed[content_start..content_start + end_idx]
                .trim()
                .to_string();
        }
    }
    trimmed.to_string()
}

/// Parse raw LLM output into a structured ReviewReport.
pub fn parse_review_report(raw: &str) -> Result<ReviewReport, ReviewError> {
    let cleaned = clean_json_response(raw);
    let value: serde_json::Value =
        serde_json::from_str(&cleaned).map_err(|e| ReviewError::Json(e.to_string()))?;

    match value {
        serde_json::Value::Object(_) => {
            let mut report: ReviewReport =
                serde_json::from_value(value).map_err(|e| ReviewError::Json(e.to_string()))?;
            if report.summary.trim().is_empty() {
                report.summary = format!("Review generated {} findings", report.findings.len());
            }
            let has_high_or_crit = report
                .findings
                .iter()
                .any(|f| matches!(f.severity, Severity::Critical | Severity::High));
            if has_high_or_crit {
                report.has_blocking_issues = true;
            }
            Ok(report)
        }
        serde_json::Value::Array(_) => {
            let findings: Vec<ReviewFinding> =
                serde_json::from_value(value).map_err(|e| ReviewError::Json(e.to_string()))?;
            let count = findings.len();
            let summary = format!("Review generated {} findings", count);
            Ok(ReviewReport::new(summary, findings))
        }
        _ => Err(ReviewError::Json(
            "Expected JSON object or array representing review report".to_string(),
        )),
    }
}

/// Execute an adversarial code review against the provided git diff.
pub fn execute_review(
    repo_root: &Path,
    diff: &str,
    task_instruction: Option<&str>,
    config: &ReviewConfig,
) -> Result<ReviewReport, ReviewError> {
    if diff.trim().is_empty() {
        return Ok(ReviewReport::clean("No changes detected in diff."));
    }

    let driver_cmd = resolve_review_driver(config.driver.as_deref())?;

    let mut prompt = String::new();
    prompt.push_str(build_review_system_prompt());
    prompt.push_str("\n\n");
    if let Some(instructions) = task_instruction {
        if !instructions.trim().is_empty() {
            prompt.push_str("Task Instructions:\n");
            prompt.push_str(instructions.trim());
            prompt.push_str("\n\n");
        }
    }
    prompt.push_str("Git Diff:\n");
    prompt.push_str(diff);

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&driver_cmd)
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("SUPERSTATE_PROMPT", &prompt)
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        let prompt_bytes = prompt.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&prompt_bytes);
        });
    }

    let output = child.wait_with_output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // If driver produced valid JSON in stdout despite non-zero exit, parse it
        if let Ok(report) = parse_review_report(&stdout) {
            return Ok(report);
        }
        return Err(ReviewError::Driver(format!(
            "Driver exited with status {}: {}{}",
            output.status.code().unwrap_or(-1),
            stderr.trim(),
            if !stdout.trim().is_empty() {
                format!(" (stdout: {})", stdout.trim())
            } else {
                String::new()
            }
        )));
    }

    parse_review_report(&stdout)
}
