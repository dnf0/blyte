//! Plan/state reconciliation and diagnostics.
//!
//! These commands keep an execution state usable across the life of a plan: the
//! plan gets revised mid-flight, states drift from the plan that produced them,
//! and a resumed session needs to know where it left off without replaying the
//! conversation that got it there.

use crate::models::{ExecutionState, ExecutionStatus, TaskSpec};
use crate::plan_parser::parse_plan_markdown;
use serde::Serialize;

/// Outcome of reconciling a state against a revised plan.
#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub preserved_completed: usize,
    pub active_task: Option<String>,
    pub pending: usize,
}

fn label(task: &TaskSpec) -> String {
    format!("{}: {}", task.id, task.title)
}

fn is_same(a: &TaskSpec, b: &TaskSpec) -> bool {
    a.id.eq_ignore_ascii_case(&b.id)
}

/// Reconcile `state` with a revised plan, preserving completed work.
///
/// Completed tasks are never re-run: they are kept as they were even if the plan
/// no longer mentions them, because the work exists on disk regardless of what
/// the plan now says. New tasks join the pending set, and tasks dropped from the
/// plan are removed from the pending set only.
pub fn sync_with_plan(
    state: &mut ExecutionState,
    plan_markdown: &str,
) -> Result<SyncReport, String> {
    let planned = parse_plan_markdown(plan_markdown).map_err(|e| e.to_string())?;

    let completed = state.completed_tasks.clone();
    let is_completed = |t: &TaskSpec| completed.iter().any(|c| is_same(c, t));

    let known: Vec<TaskSpec> = state
        .completed_tasks
        .iter()
        .chain(state.active_task.iter())
        .chain(state.pending_tasks.iter())
        .cloned()
        .collect();

    let added: Vec<String> = planned
        .iter()
        .filter(|t| !known.iter().any(|k| is_same(k, t)))
        .map(label)
        .collect();

    let removed: Vec<String> = state
        .active_task
        .iter()
        .chain(state.pending_tasks.iter())
        .filter(|t| !planned.iter().any(|p| is_same(p, t)))
        .map(label)
        .collect();

    // Everything the plan still asks for, minus what is already done.
    let mut outstanding: Vec<TaskSpec> = planned.into_iter().filter(|t| !is_completed(t)).collect();

    state.active_task = if outstanding.is_empty() {
        None
    } else {
        Some(outstanding.remove(0))
    };
    state.pending_tasks = outstanding;
    state.status = if state.active_task.is_none() {
        ExecutionStatus::Completed
    } else {
        ExecutionStatus::InProgress
    };

    Ok(SyncReport {
        added,
        removed,
        preserved_completed: state.completed_tasks.len(),
        active_task: state.active_task.as_ref().map(label),
        pending: state.pending_tasks.len(),
    })
}

/// Health of a state, optionally against the plan that produced it.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub is_healthy: bool,
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub details: DoctorDetails,
}

#[derive(Debug, Serialize)]
pub struct DoctorDetails {
    pub plan_tasks_count: usize,
    pub plan_completed_count: usize,
}

/// Diagnose drift between a state and its plan.
pub fn diagnose(state: &ExecutionState, plan_markdown: Option<&str>) -> DoctorReport {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();

    if state.active_task.is_none()
        && !state.pending_tasks.is_empty()
        && state.status != ExecutionStatus::Completed
    {
        issues.push(
            "No active task while tasks are still pending: the state machine cannot advance."
                .to_string(),
        );
    }

    // A task appearing in two sets at once means an advance went wrong somewhere.
    for done in &state.completed_tasks {
        if state.pending_tasks.iter().any(|p| is_same(p, done))
            || state.active_task.as_ref().is_some_and(|a| is_same(a, done))
        {
            issues.push(format!(
                "Task '{}' is both completed and outstanding.",
                label(done)
            ));
        }
    }

    let mut plan_tasks_count = 0;
    if let Some(markdown) = plan_markdown {
        match parse_plan_markdown(markdown) {
            Ok(planned) => {
                plan_tasks_count = planned.len();
                for t in state.active_task.iter().chain(state.pending_tasks.iter()) {
                    if !planned.iter().any(|p| is_same(p, t)) {
                        warnings.push(format!(
                            "Task '{}' is in the state but not in the plan; run `state sync`.",
                            label(t)
                        ));
                    }
                }
                for p in &planned {
                    let known = state
                        .completed_tasks
                        .iter()
                        .chain(state.active_task.iter())
                        .chain(state.pending_tasks.iter())
                        .any(|k| is_same(k, p));
                    if !known {
                        warnings.push(format!(
                            "Plan task '{}' is missing from the state; run `state sync`.",
                            label(p)
                        ));
                    }
                }
            }
            Err(e) => issues.push(format!("Plan could not be parsed: {}", e)),
        }
    }

    DoctorReport {
        is_healthy: issues.is_empty(),
        issues,
        warnings,
        details: DoctorDetails {
            plan_tasks_count,
            plan_completed_count: state.completed_tasks.len(),
        },
    }
}

/// Repair what can be repaired without guessing. Returns what it changed.
pub fn repair(state: &mut ExecutionState) -> Vec<String> {
    let mut actions = Vec::new();

    // Drop outstanding duplicates of completed work.
    let completed = state.completed_tasks.clone();
    let before = state.pending_tasks.len();
    state
        .pending_tasks
        .retain(|p| !completed.iter().any(|c| is_same(c, p)));
    if state.pending_tasks.len() != before {
        actions.push(format!(
            "Removed {} pending task(s) already completed.",
            before - state.pending_tasks.len()
        ));
    }
    if state
        .active_task
        .as_ref()
        .is_some_and(|a| completed.iter().any(|c| is_same(c, a)))
    {
        state.active_task = None;
        actions.push("Cleared an active task that was already completed.".to_string());
    }

    // Promote a pending task when nothing is active.
    if state.active_task.is_none() && !state.pending_tasks.is_empty() {
        let next = state.pending_tasks.remove(0);
        actions.push(format!("Promoted '{}' to active.", label(&next)));
        state.active_task = Some(next);
        state.status = ExecutionStatus::InProgress;
    }

    actions
}

/// Telemetry for a run in progress.
#[derive(Debug, Serialize)]
pub struct Metrics {
    pub step: usize,
    pub status: String,
    pub tasks_total: usize,
    pub tasks_completed: usize,
    pub tasks_remaining: usize,
    pub completion_pct: f64,
    pub notes_recorded: usize,
    pub errors_recorded: usize,
    pub files_modified: usize,
}

pub fn metrics(state: &ExecutionState, notes_recorded: usize) -> Metrics {
    let completed = state.completed_tasks.len();
    let remaining = state.pending_tasks.len() + usize::from(state.active_task.is_some());
    let total = completed + remaining;
    Metrics {
        step: state.step,
        status: format!("{:?}", state.status),
        tasks_total: total,
        tasks_completed: completed,
        tasks_remaining: remaining,
        completion_pct: if total == 0 {
            0.0
        } else {
            (completed as f64 / total as f64 * 1000.0).round() / 10.0
        },
        notes_recorded,
        errors_recorded: state.errors.len(),
        files_modified: state.modified_files.len(),
    }
}

/// A briefing that lets a fresh session pick up where the last one stopped.
///
/// This exists because the context that produced the current state is gone after
/// compaction; the state file is the only thing that survives, so it has to be
/// readable on its own.
pub fn resume_briefing(
    state: &ExecutionState,
    notes: &[crate::models::LedgerEntry],
    recent_commits: &[String],
) -> String {
    resume_briefing_with_root(state, notes, recent_commits, None)
}

/// A briefing that lets a fresh session pick up where the last one stopped,
/// optionally specifying an explicit repository root to locate active handoff packages.
pub fn resume_briefing_with_root(
    state: &ExecutionState,
    notes: &[crate::models::LedgerEntry],
    recent_commits: &[String],
    repo_root: Option<&std::path::Path>,
) -> String {
    let mut out = String::from("# Agent Execution Resume Briefing\n\n");
    out.push_str(&format!(
        "- **Current Status:** `{:?}` (Step {})\n",
        state.status, state.step
    ));
    if let Some(ref plan) = state.plan_path {
        out.push_str(&format!("- **Plan Path:** `{}`\n", plan.display()));
    }
    match state.active_task.as_ref() {
        Some(t) => out.push_str(&format!("- **Active Task:** {}\n", label(t))),
        None => out.push_str("- **Active Task:** none\n"),
    }
    out.push_str(&format!(
        "- **Progress:** {} completed, {} remaining\n",
        state.completed_tasks.len(),
        state.pending_tasks.len() + usize::from(state.active_task.is_some())
    ));

    // Resolve handoff artifact location. Checks the provided repo root first,
    // then falls back to project root discovered via enclosing_state_root, and finally current dir.
    let handoff_path = repo_root
        .map(|r| r.join(".agent-state/handoff.md"))
        .unwrap_or_else(|| {
            if let Some(root) =
                crate::state_machine::enclosing_state_root(std::path::Path::new("."))
            {
                let candidate = root.join(".agent-state/handoff.md");
                if candidate.exists() {
                    return candidate;
                }
            }
            std::path::PathBuf::from(".agent-state/handoff.md")
        });

    // If an active handoff artifact exists, highlight turnkey resumption instructions
    // to prevent receiving agents from having to rediscover state, worktree, and driver options.
    if handoff_path.exists() {
        out.push_str("\n## 🤝 Active Handoff\n");
        out.push_str(&format!("- **Package:** `{}`\n", handoff_path.display()));
        let worktree = state.active_worktree.as_deref();
        // Resolve state path using repo_root and active_branch via branch_scoped_state_path,
        // never using plan_path (which is markdown), falling back to .agent-state/state.json.
        let state_path = match (&state.active_branch, repo_root) {
            (Some(branch), Some(root)) => {
                crate::state_machine::branch_scoped_state_path(root, branch)
            }
            (Some(branch), None) => {
                let base = crate::state_machine::enclosing_state_root(std::path::Path::new("."));
                let root = match base.as_deref() {
                    Some(b) if b == std::path::Path::new(".") => std::path::Path::new(""),
                    Some(b) => b,
                    None => std::path::Path::new(""),
                };
                crate::state_machine::branch_scoped_state_path(root, branch)
            }
            (None, Some(root)) => root.join(".agent-state/state.json"),
            (None, None) => std::path::PathBuf::from(".agent-state/state.json"),
        };
        let claude_cmd =
            crate::handoff::generate_resumption_command("claude", worktree, &state_path);
        let agy_cmd = crate::handoff::generate_resumption_command("agy", worktree, &state_path);
        let daemon_cmd =
            crate::handoff::generate_resumption_command("daemon", worktree, &state_path);
        out.push_str(&format!("- **Claude Resume:** `{}`\n", claude_cmd));
        out.push_str(&format!("- **Agy Resume:** `{}`\n", agy_cmd));
        out.push_str(&format!("- **Daemon Resume:** `{}`\n", daemon_cmd));
    }

    if !notes.is_empty() {
        out.push_str("\n## Recorded Findings\n");
        for n in notes.iter().rev().take(5) {
            out.push_str(&format!("- [{}] {}\n", n.author, n.note));
        }
    }
    if !recent_commits.is_empty() {
        out.push_str("\n## Recent Git Commits\n");
        for c in recent_commits {
            out.push_str(&format!("- `{}`\n", c));
        }
    }
    if let Some(t) = state.active_task.as_ref() {
        out.push_str("\n## Turnkey Next Actions\n");
        out.push_str(&format!(
            "1. Generate prompt: `blyte state prompt --task \"{}\" --with-graph`\n",
            label(t)
        ));
        out.push_str(
            "2. Advance step on success: `blyte state next --verify-cmd \"<test_cmd>\" --verify-git-clean`\n",
        );
    }
    out
}
