use std::path::{Path, PathBuf};
use superstate_core::ledger::append_ledger_note;
use superstate_core::models::LedgerEntry;
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::state_machine::{advance_step, load_state, save_state_atomic};
use superstate_core::verifier::execute_all_gates;

pub fn cmd_init(plan: PathBuf, state_path: PathBuf) -> Result<(), String> {
    let state = init_state_from_plan(&plan, &state_path)
        .map_err(|e| format!("Failed to initialize state from plan: {}", e))?;
    println!("State initialized at {}", state_path.display());
    if let Some(task) = state.active_task {
        println!("Active Task: {}: {}", task.id, task.title);
    }
    Ok(())
}

/// Print tasks whose dependencies are satisfied and may therefore run now.
///
/// `--json` emits an array of titles, which is the form a runner consumes when
/// deciding what to dispatch next.
pub fn cmd_ready(state_path: &Path, as_json: bool) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let ready = superstate_core::state_machine::ready_tasks(&state);

    if as_json {
        // "Task 1: Title" -- the same form the reference implementation emits, and
        // the form a runner feeds back to `state prompt --task`. Emitting the bare
        // title would look right and fail to match.
        let titles: Vec<String> = ready
            .iter()
            .map(|t| format!("{}: {}", t.id, t.title))
            .collect();
        let rendered = serde_json::to_string_pretty(&titles)
            .map_err(|e| format!("Failed to serialize ready tasks: {}", e))?;
        println!("{}", rendered);
        return Ok(());
    }

    if ready.is_empty() {
        println!("No unblocked tasks: every pending task is waiting on an incomplete dependency.");
        return Ok(());
    }
    println!("Unblocked tasks ({}):", ready.len());
    for task in ready {
        println!("  {}: {}", task.id, task.title);
    }
    Ok(())
}

/// Find a task by id or title across the active and pending sets.
///
/// Matching is case-insensitive and accepts either the bare id ("Task 2"), the
/// bare title, or the combined "Task 2: Title" form that `state ready` emits --
/// a runner should be able to feed `ready` output straight back in.
fn select_task<'a>(
    state: &'a superstate_core::models::ExecutionState,
    wanted: Option<&str>,
) -> Option<&'a superstate_core::models::TaskSpec> {
    let wanted = match wanted {
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

pub fn cmd_status(state_path: &Path, as_json: bool) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    if as_json {
        let rendered = serde_json::to_string_pretty(&state)
            .map_err(|e| format!("Failed to serialize state: {}", e))?;
        println!("{}", rendered);
        return Ok(());
    }
    println!("State Status: {:?}", state.status);
    println!("Step: {}", state.step);
    if let Some(ref task) = state.active_task {
        println!("Active Task: {}: {}", task.id, task.title);
    } else {
        println!("Active Task: None");
    }
    println!("Completed Tasks: {}", state.completed_tasks.len());
    for task in &state.completed_tasks {
        println!("  ✓ {}: {}", task.id, task.title);
    }
    if !state.notes.is_empty() {
        println!("\nRecent Notes (Epistemic Ledger):");
        for note in state.notes.iter().rev().take(5) {
            println!("  • [{}] {}", note.author, note.note);
        }
    }
    Ok(())
}

pub fn cmd_next(
    state_path: &Path,
    verify_cmd: Option<String>,
    verify_git_clean: bool,
) -> Result<(), String> {
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let active_task = match state.active_task.clone() {
        Some(t) => t,
        None => return Err("No active task to advance".to_string()),
    };

    let mut task_to_verify = active_task;
    if let Some(cmd) = verify_cmd {
        task_to_verify.verification_cmd = Some(cmd);
    }

    let repo_root = state_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| Path::new("."));
    let results = execute_all_gates(repo_root, &task_to_verify, verify_git_clean)
        .map_err(|e| format!("Gate execution error: {}", e))?;

    for res in &results {
        if !res.passed {
            return Err(format!(
                "Verification gate '{}' FAILED:\n{}",
                res.gate_name,
                res.failure_reason.as_deref().unwrap_or(&res.output)
            ));
        }
    }

    advance_step(&mut state).map_err(|e| format!("Failed to advance step: {}", e))?;
    save_state_atomic(state_path, &state).map_err(|e| format!("Failed to save state: {}", e))?;

    if let Some(ref next_task) = state.active_task {
        println!(
            "State advanced: active_task={}: {} (status={:?})",
            next_task.id, next_task.title, state.status
        );
    } else {
        println!(
            "State completed: all tasks finished (status={:?})",
            state.status
        );
    }

    Ok(())
}

/// Generate the turnkey prompt for a task.
///
/// `task` selects a specific task by id or title -- without it only the active
/// task is reachable, which makes `state ready` a query with nowhere to go: a
/// runner can learn which tasks are unblocked but cannot ask for any of them.
pub fn cmd_prompt(state_path: &Path, task: Option<&str>, with_graph: bool) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let active = match select_task(&state, task) {
        Some(t) => t,
        None => {
            match task {
                Some(wanted) => {
                    return Err(format!(
                        "No task matching '{}' in the active or pending set.",
                        wanted
                    ))
                }
                None => println!("All tasks completed."),
            }
            return Ok(());
        }
    };

    println!(
        "You are an Implementer Subagent working on **{}: {}**.\n",
        active.id, active.title
    );
    println!("## Task Specification");
    println!("- ID: {}", active.id);
    println!("- Title: {}", active.title);
    if !active.depends_on.is_empty() {
        println!("- Depends on: {}", active.depends_on.join(", "));
    }
    if !active.files_modify.is_empty() {
        println!("Files to modify:");
        for f in &active.files_modify {
            println!("- {}", f.display());
        }
    }
    if let Some(ref cmd) = active.verification_cmd {
        println!("Verification command: `{}`", cmd);
    }

    // Completed milestones and the epistemic ledger. This is the whole point of
    // recording notes: without replaying them here, each milestone starts blind
    // and `state note` writes to something nothing ever reads. Carrying findings
    // forward -- rather than the whole conversation -- is what keeps the context
    // footprint flat while preserving what was learned.
    if !state.completed_tasks.is_empty() {
        println!("\n## Completed Milestones");
        for done in &state.completed_tasks {
            println!("- {}: {}", done.id, done.title);
        }
    }

    if !state.notes.is_empty() {
        println!("\n## Epistemic Ledger (findings from earlier milestones)");
        for entry in &state.notes {
            println!("- [{}] {}", entry.author, entry.note);
        }
    }

    if with_graph {
        let graph = find_graph_context(state_path);
        match graph {
            Some((path, body)) => {
                println!("\n## Architecture Context ({})", path.display());
                println!("{}", body);
            }
            None => {
                // Say so rather than accept the flag and silently ignore it.
                eprintln!(
                    "warning: --with-graph requested but no knowledge graph was found \
                     (looked for graphify-out/GRAPH_REPORT.md); prompt omits graph context"
                );
            }
        }
    }

    println!("\n## Instructions");
    println!("1. Implement the requested task following Test-Driven Development (TDD).");
    println!("2. Run the targeted verification command to confirm your fix.");
    println!("3. Commit your changes with a conventional commit message.");
    println!("4. Return a summary with commit SHA and verification output.");
    Ok(())
}

/// Locate a knowledge-graph report near the workspace, if one has been generated.
fn find_graph_context(state_path: &Path) -> Option<(std::path::PathBuf, String)> {
    // .agent-state/state.json -> workspace root is two levels up.
    let root = state_path.parent().and_then(|p| p.parent())?;
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

pub fn cmd_note(state_path: &Path, note: &str, author: &str) -> Result<(), String> {
    let entry = LedgerEntry::new(author.to_string(), note.to_string());
    append_ledger_note(state_path, entry).map_err(|e| format!("Failed to append note: {}", e))?;
    println!("Note recorded into epistemic ledger.");
    Ok(())
}
