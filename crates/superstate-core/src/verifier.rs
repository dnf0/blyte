use crate::models::TaskSpec;
use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GateError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Process execution error: {0}")]
    Execution(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateResult {
    pub gate_name: String,
    pub passed: bool,
    pub output: String,
    pub failure_reason: Option<String>,
}

pub fn run_verification_cmd(
    cmd: &str,
    cwd: &Path,
    _timeout_secs: u64,
) -> Result<GateResult, GateError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined_output = format!("{}{}", stdout, stderr);

    let passed = output.status.success();
    let failure_reason = if !passed {
        Some(format!(
            "Command exited with status {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ))
    } else {
        None
    };

    Ok(GateResult {
        gate_name: format!("verify_cmd: {}", cmd),
        passed,
        output: combined_output,
        failure_reason,
    })
}

pub fn verify_git_clean(repo_root: &Path) -> Result<GateResult, GateError> {
    verify_git_clean_filtered(repo_root, None)
}

pub fn verify_git_clean_filtered(
    repo_root: &Path,
    ignore_patterns: Option<&str>,
) -> Result<GateResult, GateError> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Ok(GateResult {
            gate_name: "git_clean".to_string(),
            passed: false,
            output: stderr.clone(),
            failure_reason: Some(format!("git status failed: {}", stderr)),
        });
    }

    let patterns: Vec<&str> = ignore_patterns
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let dirty_entries: Vec<&str> = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| {
            if patterns.is_empty() {
                return true;
            }
            let filename = l.split_whitespace().nth(1).unwrap_or(l);
            !patterns
                .iter()
                .any(|pat| filename.contains(pat) || filename == *pat)
        })
        .collect();

    let passed = dirty_entries.is_empty();
    let failure_reason = if !passed {
        Some(format!(
            "Working tree is not clean ({} uncommitted files):\n{}",
            dirty_entries.len(),
            stdout
        ))
    } else {
        None
    };

    Ok(GateResult {
        gate_name: "git_clean".to_string(),
        passed,
        output: stdout,
        failure_reason,
    })
}

pub fn execute_all_gates(
    repo_root: &Path,
    task: &TaskSpec,
    check_clean: bool,
) -> Result<Vec<GateResult>, GateError> {
    execute_all_gates_filtered(repo_root, task, check_clean, None)
}

pub fn execute_all_gates_filtered(
    repo_root: &Path,
    task: &TaskSpec,
    check_clean: bool,
    ignore_patterns: Option<&str>,
) -> Result<Vec<GateResult>, GateError> {
    let mut results = Vec::new();

    if check_clean {
        results.push(verify_git_clean_filtered(repo_root, ignore_patterns)?);
    }

    if let Some(ref cmd) = task.verification_cmd {
        results.push(run_verification_cmd(cmd, repo_root, 60)?);
    }

    Ok(results)
}
