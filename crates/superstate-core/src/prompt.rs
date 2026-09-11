use crate::ledger::read_ledger;
use crate::models::{ExecutionState, TaskSpec};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Compact a raw epistemic ledger note to keep turnkey prompts bounded and fast.
pub fn compact_ledger_note(note: &str) -> String {
    let clean = note.trim();
    // If multiline, take up to the first double-newline paragraph break
    let paragraph = if let Some(idx) = clean.find("\n\n") {
        clean[..idx].trim()
    } else {
        clean
    };
    // Replace embedded newlines with spaces
    let single_line = paragraph.replace('\n', " ");
    let max_len = 250;
    if single_line.len() > max_len {
        let mut end = max_len;
        while !single_line.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", single_line[..end].trim_end())
    } else {
        single_line
    }
}

/// Select a task from active or pending tasks matching the requested ID or title.
pub fn select_task<'a>(state: &'a ExecutionState, task: Option<&str>) -> Option<&'a TaskSpec> {
    match task {
        None => state.active_task.as_ref(),
        Some(wanted) => {
            if let Some(ref t) = state.active_task {
                if t.id == wanted || t.title == wanted {
                    return Some(t);
                }
            }
            state
                .pending_tasks
                .iter()
                .find(|t| t.id == wanted || t.title == wanted)
        }
    }
}

/// Resolve repository root given a state path.
fn repo_root_for(state_path: &Path) -> PathBuf {
    let root = state_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| Path::new(""));
    if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root.to_path_buf()
    }
}

/// Locate a knowledge-graph report near the workspace if generated.
fn find_graph_context(state_path: &Path) -> Option<(PathBuf, String)> {
    let root = repo_root_for(state_path);
    for candidate in [
        root.join("graphify-out/GRAPH_REPORT.md"),
        root.join("GRAPH_REPORT.md"),
    ] {
        if let Ok(body) = std::fs::read_to_string(&candidate) {
            // Keep the prompt bounded: a graph report can be very large.
            let clipped: String = body.chars().take(8000).collect();
            return Some((candidate, clipped));
        }
    }
    None
}

/// Render a self-contained turnkey prompt for an implementer subagent.
///
/// # Arguments
/// * `state` - Current execution state containing active/completed tasks and worktree info.
/// * `state_path` - Path to state.json used for ledger discovery.
/// * `task` - Optional specific task selector.
/// * `is_in_place` - Whether execution runs directly in workspace without worktree isolation.
/// * `with_graph` - Whether to append knowledge-graph architecture context.
pub fn render_turnkey_prompt(
    state: &ExecutionState,
    state_path: &Path,
    task: Option<&str>,
    is_in_place: bool,
    with_graph: bool,
) -> Result<String, String> {
    let active = match select_task(state, task) {
        Some(t) => t,
        None => {
            return match task {
                Some(wanted) => Err(format!(
                    "No task matching '{}' in the active or pending set.",
                    wanted
                )),
                None => Ok("All tasks completed.".to_string()),
            };
        }
    };

    let mut rendered = String::new();

    rendered.push_str(&format!(
        "You are an Implementer Subagent working on **{}: {}**.\n\n",
        active.id, active.title
    ));
    rendered.push_str("## Task Specification\n");
    rendered.push_str(&format!("- Task: {}: {}\n", active.id, active.title));
    rendered.push_str(&format!("- ID: {}\n", active.id));
    rendered.push_str(&format!("- Title: {}\n", active.title));
    if !active.depends_on.is_empty() {
        rendered.push_str(&format!("- Depends on: {}\n", active.depends_on.join(", ")));
    }
    if !active.files_modify.is_empty() {
        rendered.push_str("Files to modify:\n");
        let mut seen = HashSet::new();
        for f in &active.files_modify {
            if seen.insert(f) {
                rendered.push_str(&format!("- {}\n", f.display()));
            }
        }
    }
    if let Some(ref cmd) = active.verification_cmd {
        rendered.push_str(&format!("Verification command: `{}`\n", cmd));
    }

    if !is_in_place {
        if let (Some(ref wt_path), Some(ref wt_branch)) =
            (&state.active_worktree, &state.active_branch)
        {
            rendered.push_str(&format!(
                "\n## 🌲 Active Isolated Worktree\n- Worktree Path: {}\n- Branch: {}\n- Working Directory Contract: All code edits and test runs MUST be performed inside this worktree! Do NOT edit or create files in the parent workspace directory.\n- Lifecycle Contract: Do NOT attempt to commit, merge, or delete this worktree. Simply implement the task and verify it. The orchestrator automatically snapshots your changes, verifies your work, and merges this worktree into the base branch after your turn completes.\n- Container / Sandbox Paths: If task specifications or commands reference container root paths such as `/app/<path>`, map them to `<path>` relative to this worktree.\n",
                wt_path.display(),
                wt_branch
            ));
        }
    }

    if !state.completed_tasks.is_empty() {
        rendered.push_str("\n## Completed Milestones\n");
        for done in &state.completed_tasks {
            rendered.push_str(&format!("- {}: {}\n", done.id, done.title));
        }
    }

    let ledger = read_ledger(state_path);
    if !ledger.is_empty() {
        rendered.push_str("\n## Epistemic Ledger (findings from earlier milestones)\n");
        // Limit to the most recent 4 entries to keep prompt context bounded
        let start_idx = ledger.len().saturating_sub(4);
        for entry in &ledger[start_idx..] {
            rendered.push_str(&format!(
                "- [{}] {}\n",
                entry.author,
                compact_ledger_note(&entry.note)
            ));
        }
    }

    if with_graph {
        if let Some((path, body)) = find_graph_context(state_path) {
            rendered.push_str(&format!(
                "\n## Architecture Context ({})\n{}\n",
                path.display(),
                body
            ));
        }
    }

    rendered.push_str("\n## Instructions\n");
    rendered.push_str(
        "1. Implement the minimal direct fix required to satisfy the task specification.\n",
    );
    rendered.push_str("2. Run the targeted verification command to confirm your fix.\n");
    rendered.push_str("3. Hard stop upon passing (HARD STOP UPON PASSING): The moment the verification command passes with exit code 0, finish your turn immediately. Do NOT run extra tests, alternative scripts, or exploratory commands.\n");
    rendered.push_str("4. Return a concise summary with verification output.\n");
    rendered.push_str("5. Confine ALL inspections, searches, and commands strictly to the project container or workspace directory. Do not inspect parent directories, host processes, or external repositories.\n");

    Ok(rendered)
}
