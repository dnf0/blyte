use std::path::{Path, PathBuf};
use superstate_core::ledger::append_ledger_note;
use superstate_core::models::LedgerEntry;
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::state_machine::{advance_step, load_state, save_state_atomic};
use superstate_core::verifier::execute_all_gates;

/// Refuse to create a second state inside a project that already has one.
///
/// Two states for one project diverge silently: one records the work, the other
/// is consulted, and neither is obviously wrong when read on its own.
fn reject_nested_state(state_path: &Path) -> Result<(), String> {
    if state_path.is_absolute() || state_path.exists() {
        return Ok(());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match superstate_core::state_machine::enclosing_state_root(&cwd) {
        Some(root) if root != cwd => Err(format!(
            "This project already has state at {}. Creating another here would give \
             one project two diverging states. Run from {} to reinitialise it, or \
             pass an explicit --state path if a second one is genuinely intended.",
            root.join(superstate_core::state_machine::STATE_DIR)
                .join("state.json")
                .display(),
            root.display(),
        )),
        _ => Ok(()),
    }
}

pub fn cmd_init(plan: PathBuf, state_path: PathBuf) -> Result<(), String> {
    reject_nested_state(&state_path)?;
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

/// Tag the current commit as the checkpoint guarding this step.
pub fn cmd_checkpoint(state_path: &Path) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);
    let tag = superstate_core::checkpoint::create_checkpoint(&root, state.step)
        .map_err(|e| format!("Failed to create checkpoint: {}", e))?;
    println!("Checkpoint created: {}", tag);
    Ok(())
}

/// Restore the working tree to a checkpoint, discarding untracked files with it.
pub fn cmd_rollback(state_path: &Path, reference: Option<&str>) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);
    let target = superstate_core::checkpoint::rollback_to(
        &root,
        state.step,
        reference,
        &preserved(state_path),
    )
    .map_err(|e| format!("Failed to roll back: {}", e))?;
    println!("Working tree cleanly rolled back to {}", target);
    Ok(())
}

/// Paths a rollback must never delete: the state file and its directory.
fn preserved(state_path: &Path) -> Vec<&str> {
    let mut keep = vec![".agent-state"];
    if let Some(name) = state_path.file_name().and_then(|n| n.to_str()) {
        keep.push(name);
    }
    keep
}

/// The repository the state file belongs to (.agent-state/state.json -> root).
///
/// A relative state path such as `.agent-state/state.json` walks up to an empty
/// path, which is not a usable working directory, so an empty result means "here".
fn repo_root_for(state_path: &Path) -> std::path::PathBuf {
    let root = state_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| Path::new(""));
    if root.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        root.to_path_buf()
    }
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
    println!(
        "Ledger Entries: {}",
        superstate_core::ledger::read_ledger(state_path).len()
    );
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

    let repo_root = repo_root_for(state_path);
    let results = execute_all_gates(&repo_root, &task_to_verify, verify_git_clean)
        .map_err(|e| format!("Gate execution error: {}", e))?;

    for res in &results {
        if !res.passed {
            // Record why the gate refused, in the state rather than only in the
            // caller's scrollback. A run resumed after context loss has nothing
            // but this file to explain why it stopped here.
            let reason = res
                .failure_reason
                .clone()
                .unwrap_or_else(|| res.output.clone());
            state.errors.push(reason.clone());
            save_state_atomic(state_path, &state)
                .map_err(|e| format!("Failed to record gate failure: {}", e))?;

            return Err(format!(
                "Verification gate '{}' FAILED:\n{}",
                res.gate_name, reason
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

    // A state written before the ledger was split still carries its notes
    // inline; move them across rather than silently reading an empty ledger.
    migrate_ledger_if_needed(state_path);
    let ledger = superstate_core::ledger::read_ledger(state_path);
    if !ledger.is_empty() {
        println!("\n## Epistemic Ledger (findings from earlier milestones)");
        for entry in &ledger {
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

/// Move notes out of a pre-split state document, once, on first touch.
fn migrate_ledger_if_needed(state_path: &Path) {
    match superstate_core::ledger::migrate_inline_notes(state_path) {
        Ok(0) => {}
        Ok(n) => log_migration(n, state_path),
        Err(err) => eprintln!("warning: could not migrate inline ledger notes: {}", err),
    }
}

fn log_migration(count: usize, state_path: &Path) {
    eprintln!(
        "note: moved {} ledger entr{} out of {} into {}",
        count,
        if count == 1 { "y" } else { "ies" },
        state_path.display(),
        superstate_core::ledger::ledger_path(state_path).display(),
    );
}

pub fn cmd_note(state_path: &Path, note: &str, author: &str) -> Result<(), String> {
    migrate_ledger_if_needed(state_path);
    let entry = LedgerEntry::new(author.to_string(), note.to_string());
    append_ledger_note(state_path, entry).map_err(|e| format!("Failed to append note: {}", e))?;
    println!("Note recorded into epistemic ledger.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Reconciliation, diagnostics and patching
// ---------------------------------------------------------------------------

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&body).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
    }
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

/// Reconcile the state with a revised plan, preserving completed work.
pub fn cmd_sync(state_path: &Path, plan_path: &Path, out: Option<&Path>) -> Result<(), String> {
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let plan = std::fs::read_to_string(plan_path)
        .map_err(|e| format!("Failed to read plan {}: {}", plan_path.display(), e))?;

    let report = superstate_core::maintenance::sync_with_plan(&mut state, &plan)?;
    state.plan_path = Some(plan_path.to_path_buf());
    write_json(
        out.unwrap_or(state_path),
        &superstate_core::patching::state_to_value(&state)?,
    )?;

    println!(
        "State synced with plan '{}': active_task={} (pending={})",
        plan_path.display(),
        report.active_task.as_deref().unwrap_or("none"),
        report.pending
    );
    for added in &report.added {
        println!("  + {}", added);
    }
    for removed in &report.removed {
        println!("  - {}", removed);
    }
    if report.preserved_completed > 0 {
        println!(
            "  {} completed task(s) preserved.",
            report.preserved_completed
        );
    }
    Ok(())
}

/// Diagnose plan/state drift, optionally repairing what can be repaired.
pub fn cmd_doctor(
    state_path: &Path,
    plan_path: Option<&Path>,
    repair: bool,
    as_json: bool,
) -> Result<(), String> {
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let plan = match plan_path {
        Some(p) => {
            Some(std::fs::read_to_string(p).map_err(|e| format!("Failed to read plan: {}", e))?)
        }
        None => None,
    };

    let report = superstate_core::maintenance::diagnose(&state, plan.as_deref());
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!("Healthy: {}", report.is_healthy);
        for i in &report.issues {
            println!("  issue:   {}", i);
        }
        for w in &report.warnings {
            println!("  warning: {}", w);
        }
    }

    if repair {
        let actions = superstate_core::maintenance::repair(&mut state);
        if actions.is_empty() {
            println!("Nothing to repair.");
        } else {
            write_json(
                state_path,
                &superstate_core::patching::state_to_value(&state)?,
            )?;
            for a in actions {
                println!("  repaired: {}", a);
            }
        }
    }
    Ok(())
}

/// Validate a state document against the execution schema.
pub fn cmd_validate(state_path: &Path) -> Result<(), String> {
    let raw = read_json(state_path)?;
    let errors = superstate_core::patching::validate_state(&raw);
    if errors.is_empty() {
        println!("State is valid.");
        return Ok(());
    }
    // Report every problem at once: one at a time turns this into a guessing game.
    for e in &errors {
        eprintln!("  invalid: {}", e);
    }
    Err(format!("State is invalid ({} problem(s)).", errors.len()))
}

/// Apply a JSON patch to the state, and/or append a note.
pub fn cmd_apply(
    state_path: &Path,
    patch_path: Option<&Path>,
    note: Option<&str>,
    out: Option<&Path>,
) -> Result<(), String> {
    if patch_path.is_none() && note.is_none() {
        return Err("Nothing to apply: pass --patch and/or --note.".to_string());
    }
    let mut state = read_json(state_path)?;

    if let Some(patch_path) = patch_path {
        let patch = read_json(patch_path)?;
        superstate_core::patching::apply_patch(&mut state, &patch);
    }
    if let Some(note) = note {
        superstate_core::patching::append_note(&mut state, "agent", note);
    }

    // Refuse to write a state we would not accept on read.
    let errors = superstate_core::patching::validate_state(&state);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("  invalid: {}", e);
        }
        return Err("Patch would produce an invalid state; nothing written.".to_string());
    }

    write_json(out.unwrap_or(state_path), &state)?;
    println!("State patch applied.");
    Ok(())
}

/// Executive briefing for resuming after context loss.
pub fn cmd_resume(state_path: &Path) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);
    let commits = std::process::Command::new("git")
        .args(["log", "--oneline", "-5"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    migrate_ledger_if_needed(state_path);
    let notes = superstate_core::ledger::read_ledger(state_path);
    println!(
        "{}",
        superstate_core::maintenance::resume_briefing(&state, &notes, &commits)
    );
    Ok(())
}

/// Task telemetry and efficiency metrics.
pub fn cmd_metrics(state_path: &Path, as_json: bool) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    migrate_ledger_if_needed(state_path);
    let notes = superstate_core::ledger::read_ledger(state_path);
    let m = superstate_core::maintenance::metrics(&state, notes.len());
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?
        );
    } else {
        println!("Step:            {}", m.step);
        println!("Status:          {}", m.status);
        println!(
            "Tasks:           {}/{} complete ({}%)",
            m.tasks_completed, m.tasks_total, m.completion_pct
        );
        println!("Remaining:       {}", m.tasks_remaining);
        println!("Notes recorded:  {}", m.notes_recorded);
        println!("Errors recorded: {}", m.errors_recorded);
    }
    Ok(())
}

/// Decompose coarse plan tasks into atomic child micro-tasks.
pub fn cmd_decompose(plan_path: &Path, out: Option<&Path>, in_place: bool) -> Result<(), String> {
    let plan = std::fs::read_to_string(plan_path)
        .map_err(|e| format!("Failed to read plan {}: {}", plan_path.display(), e))?;
    let decomposed = superstate_core::patching::decompose_plan(&plan)?;

    match (in_place, out) {
        (true, _) => {
            std::fs::write(plan_path, &decomposed).map_err(|e| e.to_string())?;
            println!("Plan decomposed in place: {}", plan_path.display());
        }
        (false, Some(dest)) => {
            std::fs::write(dest, &decomposed).map_err(|e| e.to_string())?;
            println!("Decomposed plan written to {}", dest.display());
        }
        (false, None) => print!("{}", decomposed),
    }
    Ok(())
}

/// Run a verification command and atomically advance, or record the failure.
///
/// This collapses checkpoint -> verify -> advance-or-rollback into one operation,
/// so a run cannot end up half-advanced: either the gate passed and the state
/// moved on, or it failed and the state records why. Optionally restores the
/// checkpoint on failure, leaving no debris for the next attempt.
#[allow(clippy::too_many_arguments)]
pub fn cmd_exec(
    state_path: &Path,
    verify_cmd: &str,
    verify_git_clean: bool,
    checkpoint: bool,
    rollback_on_fail: bool,
    advance: bool,
    as_json: bool,
) -> Result<(), String> {
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);
    let step = state.step;

    if checkpoint {
        superstate_core::checkpoint::create_checkpoint(&root, step)
            .map_err(|e| format!("Failed to checkpoint before exec: {}", e))?;
    }

    let gate = superstate_core::verifier::run_verification_cmd(verify_cmd, &root, 600)
        .map_err(|e| format!("Verification could not run: {}", e))?;

    let mut git_clean_passed = true;
    if gate.passed && verify_git_clean {
        let clean = superstate_core::verifier::verify_git_clean(&root)
            .map_err(|e| format!("git clean check failed: {}", e))?;
        git_clean_passed = clean.passed;
    }

    let passed = gate.passed && git_clean_passed;
    let mut rolled_back = false;
    let mut advanced = false;

    if passed && advance {
        cmd_next(state_path, Some(verify_cmd.to_string()), verify_git_clean)?;
        advanced = true;
    } else if !passed {
        // Record why, so the failure survives in the state rather than only in
        // whatever scrollback the caller happens to keep.
        let mut failed_state =
            load_state(state_path).map_err(|e| format!("Failed to reload state: {}", e))?;
        failed_state.errors.push(format!(
            "step {}: verification failed for `{}`: {}",
            step,
            verify_cmd,
            gate.failure_reason.clone().unwrap_or_else(|| {
                if git_clean_passed {
                    "non-zero exit".into()
                } else {
                    "working tree not clean".into()
                }
            })
        ));
        let value = superstate_core::patching::state_to_value(&failed_state)?;
        write_json(state_path, &value)?;

        if rollback_on_fail {
            superstate_core::checkpoint::rollback_to(&root, step, None, &preserved(state_path))
                .map_err(|e| format!("Failed to roll back after failure: {}", e))?;
            rolled_back = true;
        }
    }

    if as_json {
        let report = serde_json::json!({
            "passed": passed,
            "advanced": advanced,
            "rolled_back": rolled_back,
            "step": step,
            "verify_cmd": verify_cmd,
            "output": gate.output,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else if passed {
        println!(
            "Verification passed{}",
            if advanced {
                " and state advanced."
            } else {
                "."
            }
        );
    } else {
        println!(
            "Verification failed{}",
            if rolled_back {
                "; working tree rolled back."
            } else {
                "; failure recorded in state."
            }
        );
    }

    if passed {
        Ok(())
    } else {
        Err("verification failed".to_string())
    }
}
