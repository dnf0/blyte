//! Implementation of the `blyte instructions` command.

use std::path::{Path, PathBuf};
use std::process::Command;

use blyte_agent::orchestrator::Answers;
use blyte_agent::run::ORCHESTRATOR_FIXED;

use crate::orchestrator_text;

pub fn repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| "not inside a git repository".to_string())?;
    if !out.status.success() {
        return Err("not inside a git repository".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("not inside a git repository".to_string());
    }
    Ok(PathBuf::from(trimmed))
}

pub fn render(dispatch_tool: bool, answers: Answers, root: &Path) -> Result<String, String> {
    let built = orchestrator_text::build(dispatch_tool, answers, root)?;
    let mut out = String::new();
    for line in &built.start_lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&built.instructions);
    if !built.instructions.ends_with("\n\n") {
        if !built.instructions.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str("## Always sent\n");
    out.push_str(ORCHESTRATOR_FIXED);
    if !ORCHESTRATOR_FIXED.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str("## Tool descriptions\n");
    for tool in blyte_agent::tools::specs() {
        out.push_str(&format!("### {}\n{}\n\n", tool.name, tool.description));
    }
    out.push_str("The request and a description of the environment are added when a run starts.\n");
    Ok(out)
}
