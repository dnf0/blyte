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

pub fn cmd_status(state_path: &Path) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
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

pub fn cmd_prompt(state_path: &Path) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let active = match state.active_task {
        Some(t) => t,
        None => {
            println!("All tasks completed.");
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
    Ok(())
}

pub fn cmd_note(state_path: &Path, note: String, author: String) -> Result<(), String> {
    let entry = LedgerEntry::new(author, note);
    append_ledger_note(state_path, entry).map_err(|e| format!("Failed to append note: {}", e))?;
    println!("Note recorded into epistemic ledger.");
    Ok(())
}
