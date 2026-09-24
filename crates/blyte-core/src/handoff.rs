//! Standardized Cross-Agent Handoff Package & Resumption Generator.
//!
//! When human developers or AI agents (such as Claude Code or Antigravity) transition
//! execution to another agent or headless daemon, context loss and permission deadlocks
//! are frequent points of failure. This module standardizes the creation of portable,
//! self-contained handoff artifacts (`.agent-state/handoff.md` and
//! `.agent-state/handoffs/<slug>.md`) containing the high-level objective, active milestone
//! specifications, verification invariants, recent epistemic discoveries, and turnkey
//! resumption commands for the recipient agent persona.

use crate::models::{ExecutionState, LedgerEntry};
use crate::prompt::compact_ledger_note;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Specification of a cross-agent handoff event.
///
/// Encapsulates the origin persona, destination persona, state location,
/// isolated worktree directory, and job identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HandoffSpec {
    /// Agent initiating the handoff (e.g., "claude", "agy", "human").
    pub from_agent: Option<String>,
    /// Target recipient agent persona (e.g., "claude", "agy", "daemon", "any").
    pub to_agent: Option<String>,
    /// Path to the execution state JSON file.
    pub state_path: PathBuf,
    /// Path to the isolated Git worktree if task isolation is enabled.
    pub worktree_path: Option<PathBuf>,
    /// Unique identifier / branch slug for the task.
    pub slug: String,
}

/// Generates a turnkey shell command for the target agent to resume execution.
///
/// # Arguments
/// * `target_agent` - Agent persona name (e.g. "claude", "agy", "daemon", "any").
/// * `worktree_path` - Optional worktree directory; if present, prefaces commands with `cd <wt> && `.
/// * `state_path` - Path to the state file needed by daemons and fallback CLI commands.
///
/// # Returns
/// A shell-executable string command tailored to the target persona.
pub fn generate_resumption_command(
    target_agent: &str,
    worktree_path: Option<&Path>,
    state_path: &Path,
) -> String {
    // If working in an isolated worktree, the receiving agent must first navigate to
    // that directory to ensure file modifications and verification checks occur in isolation.
    let cd_prefix = match worktree_path {
        Some(wt) => format!("cd {} && ", wt.display()),
        None => String::new(),
    };

    // Normalize target identifier for case-insensitive matching
    match target_agent.trim().to_lowercase().as_str() {
        // Claude Code runs interactively from the target directory
        "claude" => format!("{}claude", cd_prefix),
        // Google Antigravity CLI runs interactively from the target directory
        "agy" => format!("{}agy", cd_prefix),
        // Background daemon delegates to autonomous runner with claude driver by default
        "daemon" => format!(
            "blyte state daemon --state {} --driver claude --detach",
            state_path.display()
        ),
        // Fallback for human developers or unspecified agents uses standard state resume CLI
        _ => format!(
            "{}blyte state resume --state {}",
            cd_prefix,
            state_path.display()
        ),
    }
}

/// Formats a complete, self-contained Markdown handoff document.
///
/// # Arguments
/// * `state` - Current execution state containing active milestone and global goal.
/// * `spec` - Metadata specifying source agent, target agent, paths, and slug.
/// * `notes` - Historical epistemic ledger entries capturing learnings and guardrails.
///
/// # Returns
/// Markdown document string formatted with alerts, specifications, and resumption snippets.
pub fn generate_handoff_markdown(
    state: &ExecutionState,
    spec: &HandoffSpec,
    notes: &[LedgerEntry],
) -> String {
    let from = spec.from_agent.as_deref().unwrap_or("unknown");
    let to = spec.to_agent.as_deref().unwrap_or("any");
    let working_dir = spec
        .worktree_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "repository root (in-place)".to_string());

    let active_task_str = match &state.active_task {
        Some(t) => format!("`{}`: {}", t.id, t.title),
        None => "All milestones completed".to_string(),
    };

    // Header metadata block provides quick telemetry for the receiving agent
    let mut md = String::new();
    md.push_str("# 🤝 Blyte Handoff Package\n\n");
    md.push_str(&format!("- **Source Agent**: {}\n", from));
    md.push_str(&format!("- **Target Agent**: {}\n", to));
    md.push_str(&format!("- **Job Slug**: {}\n", spec.slug));
    md.push_str(&format!(
        "- **State File**: `{}`\n",
        spec.state_path.display()
    ));
    md.push_str(&format!("- **Active Task**: {}\n", active_task_str));
    md.push_str(&format!("- **Working Directory**: `{}`\n\n", working_dir));
    md.push_str("---\n\n");

    // Surface global goal objective if recorded
    if let Some(ref goal) = state.goal {
        md.push_str("## 🎯 Global Objective\n");
        md.push_str(goal.trim());
        md.push_str("\n\n");
    }

    // Surface active milestone specification so receiving agent has invariant gates
    if let Some(ref active) = state.active_task {
        md.push_str("## 📋 Active Milestone Specification\n");
        md.push_str(&format!("- **Task ID**: `{}`\n", active.id));
        md.push_str(&format!("- **Title**: {}\n", active.title));
        if let Some(ref cmd) = active.verification_cmd {
            md.push_str(&format!("- **Verification Invariant**: `{}`\n", cmd));
        }
        if !active.files_modify.is_empty() {
            md.push_str("- **Files to Modify**:\n");
            for f in &active.files_modify {
                md.push_str(&format!("  - `{}`\n", f.display()));
            }
        }
        md.push('\n');
    }

    // Epistemic ledger entries provide critical guardrails and context from prior turns.
    // We cap to the 5 most recent entries to maintain bounded token footprint.
    if !notes.is_empty() {
        md.push_str("## 🧠 Epistemic Ledger (Discoveries & Guardrails)\n");
        let start = notes.len().saturating_sub(5);
        for entry in &notes[start..] {
            md.push_str(&format!(
                "- [{}] {}\n",
                entry.author,
                compact_ledger_note(&entry.note)
            ));
        }
        md.push('\n');
    }

    md.push_str("---\n\n");
    md.push_str("## 🚀 Ready-to-Run Resumption\n\n");

    // Generate turnkey commands for each supported transition target
    let claude_cmd =
        generate_resumption_command("claude", spec.worktree_path.as_deref(), &spec.state_path);
    let agy_cmd =
        generate_resumption_command("agy", spec.worktree_path.as_deref(), &spec.state_path);
    let daemon_cmd =
        generate_resumption_command("daemon", spec.worktree_path.as_deref(), &spec.state_path);

    // Option A: Claude Code interactive handoff instructions
    md.push_str("### Option A: If continuing with Claude Code\n");
    md.push_str("```bash\n");
    md.push_str(&claude_cmd);
    md.push_str("\n```\n");
    md.push_str("> *Prompt to send:* `Review .agent-state/handoff.md and implement the active milestone.`\n\n");

    // Option B: Google Antigravity interactive handoff instructions
    md.push_str("### Option B: If continuing with Antigravity (agy CLI / IDE)\n");
    md.push_str("```bash\n");
    md.push_str(&agy_cmd);
    md.push_str("\n```\n");
    md.push_str(
        "> *Or use the state_prompt MCP tool to load the turnkey instructions directly.*\n\n",
    );

    // Option C: Detached daemon autonomous runner instructions
    md.push_str("### Option C: If delegating to background daemon\n");
    md.push_str("```bash\n");
    md.push_str(&daemon_cmd);
    md.push_str("\n```\n");

    md
}

/// Writes the standardized handoff document both to the active handoff file
/// (`.agent-state/handoff.md`) and a scoped archive (`.agent-state/handoffs/<slug>.md`).
///
/// # Arguments
/// * `repo_root` - Root directory of the repository.
/// * `state` - Current execution state.
/// * `spec` - Handoff configuration.
/// * `notes` - Historical epistemic ledger entries.
///
/// # Returns
/// Absolute or repo-relative path to the primary `.agent-state/handoff.md` file on success,
/// or an error message on I/O failure.
pub fn write_handoff_package(
    repo_root: &Path,
    state: &ExecutionState,
    spec: &HandoffSpec,
    notes: &[LedgerEntry],
) -> Result<PathBuf, String> {
    // Generate the full markdown representation
    let md = generate_handoff_markdown(state, spec, notes);

    // Ensure the `.agent-state` parent directory exists
    let agent_state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&agent_state_dir)
        .map_err(|e| format!("Failed to create .agent-state directory: {}", e))?;

    // Write the primary active handoff package that tools and agents look for by default
    let handoff_file = agent_state_dir.join("handoff.md");
    fs::write(&handoff_file, &md)
        .map_err(|e| format!("Failed to write {}: {}", handoff_file.display(), e))?;

    // Create the scoped handoffs directory to preserve history across different task slugs
    let handoffs_dir = agent_state_dir.join("handoffs");
    fs::create_dir_all(&handoffs_dir)
        .map_err(|e| format!("Failed to create handoffs directory: {}", e))?;

    // Write the scoped handoff package for record-keeping (sanitize slug to handle branch names with slashes)
    let sanitized_slug = crate::state_machine::sanitize_branch_slug(&spec.slug);
    let scoped_file = handoffs_dir.join(format!("{}.md", sanitized_slug));
    fs::write(&scoped_file, &md)
        .map_err(|e| format!("Failed to write {}: {}", scoped_file.display(), e))?;

    Ok(handoff_file)
}
