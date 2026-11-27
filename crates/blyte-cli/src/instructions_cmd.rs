//! Implementation of the `blyte instructions` command.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use blyte_agent::approval::{ApprovalConfig, NoApprover};
use blyte_agent::conversation::Answers;
use blyte_agent::run::CONVERSATION_FIXED;

use crate::conversation_text;

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

pub fn render_with_sandbox(
    dispatch_tool: bool,
    answers: Answers,
    root: &Path,
    sandbox_on: bool,
) -> Result<String, String> {
    let allowed_in_advance = if sandbox_on {
        crate::approval_file::load()?.len()
    } else {
        0
    };
    let built = conversation_text::build_with_sandbox(
        dispatch_tool,
        answers,
        root,
        sandbox_on,
        allowed_in_advance,
    )?;
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
    out.push_str(CONVERSATION_FIXED);
    if !CONVERSATION_FIXED.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str("## Tool descriptions\n");
    let prev_approval = blyte_agent::tools::approval();
    if sandbox_on {
        blyte_agent::tools::set_approval(Some(ApprovalConfig {
            rules: Vec::new(),
            approver: Arc::new(NoApprover),
        }));
    } else {
        blyte_agent::tools::set_approval(None);
    }
    let specs = blyte_agent::tools::specs();
    blyte_agent::tools::set_approval(prev_approval);
    for tool in specs {
        out.push_str(&format!("### {}\n{}\n", tool.name, tool.description));
        if tool.name == "bash" {
            if let Some(props) = tool
                .parameters
                .get("properties")
                .and_then(|v| v.as_object())
            {
                for prop in ["unsandboxed", "reason"] {
                    if let Some(desc) = props
                        .get(prop)
                        .and_then(|v| v.get("description"))
                        .and_then(|v| v.as_str())
                    {
                        out.push_str(&format!("- `{prop}`: {desc}\n"));
                    }
                }
            }
        }
        out.push('\n');
    }
    out.push_str("The request and a description of the environment are added when a run starts.\n");
    Ok(out)
}
