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
    let wanted = match task {
        Some(w) => w.trim(),
        None => return state.active_task.as_ref(),
    };
    state
        .active_task
        .iter()
        .chain(state.pending_tasks.iter())
        .find(|t| {
            let combined = format!("{}: {}", t.id, t.title);
            t.id.eq_ignore_ascii_case(wanted)
                || t.title.eq_ignore_ascii_case(wanted)
                || combined.eq_ignore_ascii_case(wanted)
        })
}

/// Resolve repository root given a state path.
/// Walks upward until finding `.agent-state`, then returns its parent directory.
/// If `.agent-state` is not found, falls back to the state file's immediate parent directory.
pub fn resolve_project_root(state_path: &Path) -> PathBuf {
    let mut dir = state_path.parent();
    while let Some(current) = dir {
        if current.file_name().and_then(|s| s.to_str()) == Some(crate::state_machine::STATE_DIR) {
            if let Some(parent) = current.parent() {
                if parent.as_os_str().is_empty() {
                    return PathBuf::from(".");
                }
                return parent.to_path_buf();
            }
        }
        dir = current.parent();
    }
    if let Some(parent) = state_path.parent() {
        if !parent.as_os_str().is_empty() {
            return parent.to_path_buf();
        }
    }
    PathBuf::from(".")
}

/// Locate a knowledge-graph report near the workspace if generated.
fn find_graph_context(state_path: &Path) -> Option<(PathBuf, String)> {
    let root = resolve_project_root(state_path);
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

    // Inject the global objective and requirements if recorded in the state with non-empty content,
    // providing full task specification context to the subagent before the individual milestone breakdown.
    if let Some(goal) = state
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        rendered.push_str("## 🎯 Global Task Objective & Requirements\n");
        rendered.push_str(goal);
        rendered.push_str("\n\n");
    }

    rendered.push_str("## Task Specification\n");
    rendered.push_str(&format!("- Task: {}: {}\n", active.id, active.title));
    rendered.push_str(&format!("- ID: {}\n", active.id));
    rendered.push_str(&format!("- Title: {}\n", active.title));
    if !active.depends_on.is_empty() {
        rendered.push_str(&format!("- Depends on: {}\n", active.depends_on.join(", ")));
    }
    if let Some(ref verify_cmd) = active.verification_cmd {
        rendered.push_str(&format!("- Verification Command: `{}`\n", verify_cmd));
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

    if !is_in_place {
        if let (Some(ref wt_path), Some(ref wt_branch)) =
            (&state.active_worktree, &state.active_branch)
        {
            rendered.push_str(&format!(
                "\n## 🌲 Active Isolated Worktree\n- Worktree Path: {}\n- Branch: {}\n- State File: {}\n- Next Gate Command: superstate state next --state \"{}\"\n- Working Directory Contract: All code edits and test runs MUST be performed inside this worktree! Do NOT edit or create files in the parent workspace directory.\n- Lifecycle Contract: Do NOT attempt to commit, merge, or delete this worktree. Simply implement the task and verify it. The orchestrator automatically snapshots your changes, verifies your work, and merges this worktree into the base branch after your turn completes.\n- Container / Sandbox Paths: If task specifications or commands reference container root paths such as `/app/<path>`, map them to `<path>` relative to this worktree.\n",
                wt_path.display(),
                wt_branch,
                state_path.display(),
                state_path.display(),
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
        match find_graph_context(state_path) {
            Some((path, body)) => {
                rendered.push_str(&format!(
                    "\n## Architecture Context ({})\n{}\n",
                    path.display(),
                    body
                ));
            }
            None => {
                eprintln!(
                    "warning: --with-graph requested but no knowledge graph was found \
                     (looked for graphify-out/GRAPH_REPORT.md); prompt omits graph context"
                );
            }
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
