use std::path::{Path, PathBuf};
use superstate_core::ledger::append_ledger_note;
use superstate_core::models::LedgerEntry;
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::state_machine::{advance_step, load_state, save_state_atomic};
use superstate_core::verifier::execute_all_gates;
use superstate_worktree::manager::{IsolatedWorktree, WorktreeManager};

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

fn resolve_project_root(state_path: &Path) -> PathBuf {
    if let Some(parent) = state_path.parent() {
        if parent.file_name().and_then(|s| s.to_str())
            == Some(superstate_core::state_machine::STATE_DIR)
        {
            if let Some(grandparent) = parent.parent() {
                if grandparent.as_os_str().is_empty() {
                    return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                }
                return grandparent.to_path_buf();
            }
        }
        if !parent.as_os_str().is_empty() {
            return parent.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn ensure_git_baseline(repo_root: &Path) {
    let check = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(repo_root)
        .output();

    let inside_git = match check {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    };

    if !inside_git {
        let init_status = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo_root)
            .status();

        let ok = match init_status {
            Ok(s) if s.success() => true,
            _ => std::process::Command::new("git")
                .args(["init"])
                .current_dir(repo_root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
        };

        if ok {
            let _ = std::process::Command::new("git")
                .args(["config", "user.name", "Superstate"])
                .current_dir(repo_root)
                .status();
            let _ = std::process::Command::new("git")
                .args(["config", "user.email", "superstate@local"])
                .current_dir(repo_root)
                .status();
            let _ = std::process::Command::new("git")
                .args([
                    "commit",
                    "--allow-empty",
                    "--no-gpg-sign",
                    "-m",
                    "chore(superstate): initialize tracking baseline",
                ])
                .current_dir(repo_root)
                .status();
        }
    }

    ensure_git_exclude(repo_root);
}

pub fn ensure_git_exclude(repo_root: &Path) {
    let exclude_path = repo_root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let entries = [
        ".agent-state/",
        ".wt/",
        ".wt",
        "plan.md",
        "plan.normalized.md",
        "run-in-container",
    ];
    if let Ok(mut current) = std::fs::read_to_string(&exclude_path) {
        let mut appended = false;
        for entry in &entries {
            if !current.lines().any(|l| l.trim() == *entry) {
                if !current.ends_with('\n') && !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(entry);
                current.push('\n');
                appended = true;
            }
        }
        if appended {
            let _ = std::fs::write(&exclude_path, current);
        }
    } else {
        let initial = ".agent-state/\n.wt/\n.wt\nplan.md\nplan.normalized.md\nrun-in-container\n";
        let _ = std::fs::write(&exclude_path, initial);
    }
}

pub fn cmd_init(plan: PathBuf, state_path: PathBuf) -> Result<(), String> {
    reject_nested_state(&state_path)?;
    let root = resolve_project_root(&state_path);
    ensure_git_baseline(&root);
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
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);

    // If an isolated worktree was active for this step, abort it cleanly before restoring
    // the git checkpoint, preventing dangling branches or worktrees.
    if let Some(ref wt_path) = state.active_worktree.clone() {
        if wt_path.exists() {
            let base_branch = WorktreeManager::new(&root)
                .map(|m| m.default_branch())
                .unwrap_or_else(|_| "main".to_string());
            let branch = state
                .active_branch
                .clone()
                .unwrap_or_else(|| "superstate/task".to_string());
            let wt = IsolatedWorktree {
                branch,
                path: wt_path.clone(),
                base_branch,
                repo_root: root.clone(),
            };
            wt.abort()
                .map_err(|e| format!("Failed to abort worktree on rollback: {}", e))?;
        }
        state.active_worktree = None;
        state.active_branch = None;
        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to update state after worktree abort: {}", e))?;
    }

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
    if let Some(parent) = state_path.parent() {
        if parent.file_name().and_then(|s| s.to_str())
            == Some(superstate_core::state_machine::STATE_DIR)
        {
            if let Some(grandparent) = parent.parent() {
                if grandparent.as_os_str().is_empty() {
                    return std::path::PathBuf::from(".");
                }
                return grandparent.to_path_buf();
            }
        } else if parent.join(".git").exists() {
            if parent.as_os_str().is_empty() {
                return std::path::PathBuf::from(".");
            }
            return parent.to_path_buf();
        }
    }
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

    let mut task_to_verify = active_task.clone();
    if let Some(cmd) = verify_cmd {
        task_to_verify.verification_cmd = Some(cmd);
    }

    let repo_root = repo_root_for(state_path);
    ensure_git_exclude(&repo_root);

    // When an isolated worktree is active, gates must run within that worktree checkout
    // where task changes were staged and committed, rather than the primary repo root.
    let target_dir = state
        .active_worktree
        .as_ref()
        .filter(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| repo_root.clone());

    let results = execute_all_gates(&target_dir, &task_to_verify, verify_git_clean)
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

    // Merge verified worktree back into base branch and destroy the temporary checkout.
    // If merge fails, state is preserved without advancement so work is not lost.
    if let Some(ref wt_path) = state.active_worktree.clone() {
        if wt_path.exists() {
            let base_branch = WorktreeManager::new(&repo_root)
                .map(|m| m.default_branch())
                .unwrap_or_else(|_| "main".to_string());
            let branch = state
                .active_branch
                .clone()
                .unwrap_or_else(|| {
                    format!(
                        "superstate/{}",
                        superstate_worktree::manager::sanitize_slug(&active_task.id)
                    )
                });
            let wt = IsolatedWorktree {
                branch,
                path: wt_path.clone(),
                base_branch,
                repo_root: repo_root.clone(),
            };
            let commit_msg = format!("feat({}): {}", active_task.id, active_task.title);
            wt.merge_and_cleanup(&commit_msg)
                .map_err(|e| format!("Failed to merge and cleanup worktree: {}", e))?;
            state.active_worktree = None;
            state.active_branch = None;
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
pub fn cmd_prompt(
    state_path: &Path,
    task: Option<&str>,
    with_graph: bool,
    json_output: bool,
) -> Result<(), String> {
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let active = match select_task(&state, task).cloned() {
        Some(t) => t,
        None => {
            match task {
                Some(wanted) => {
                    return Err(format!(
                        "No task matching '{}' in the active or pending set.",
                        wanted
                    ))
                }
                None => {
                    if json_output {
                        let json = serde_json::json!({
                            "task": serde_json::Value::Null,
                            "active_worktree": serde_json::Value::Null,
                            "active_branch": serde_json::Value::Null,
                            "prompt": "All tasks completed."
                        });
                        let rendered = serde_json::to_string_pretty(&json)
                            .map_err(|e| format!("Failed to serialize state: {}", e))?;
                        println!("{}", rendered);
                    } else {
                        println!("All tasks completed.");
                    }
                }
            }
            return Ok(());
        }
    };

    let repo_root = repo_root_for(state_path);
    if repo_root.join(".git").exists() {
        ensure_git_exclude(&repo_root);
        let manager = WorktreeManager::new(&repo_root)
            .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
        let wt = manager
            .ensure_task_worktree(&active.id, None)
            .map_err(|e| format!("Failed to ensure worktree for task {}: {}", active.id, e))?;
        state.active_worktree = Some(wt.path.clone());
        state.active_branch = Some(wt.branch.clone());
        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to save state: {}", e))?;
    }

    let mut rendered_prompt = String::new();

    rendered_prompt.push_str(&format!(
        "You are an Implementer Subagent working on **{}: {}**.\n\n",
        active.id, active.title
    ));
    rendered_prompt.push_str("## Task Specification\n");
    rendered_prompt.push_str(&format!("- ID: {}\n", active.id));
    rendered_prompt.push_str(&format!("- Title: {}\n", active.title));
    if !active.depends_on.is_empty() {
        rendered_prompt.push_str(&format!("- Depends on: {}\n", active.depends_on.join(", ")));
    }
    if !active.files_modify.is_empty() {
        rendered_prompt.push_str("Files to modify:\n");
        for f in &active.files_modify {
            rendered_prompt.push_str(&format!("- {}\n", f.display()));
        }
    }
    if let Some(ref cmd) = active.verification_cmd {
        rendered_prompt.push_str(&format!("Verification command: `{}`\n", cmd));
    }

    if let (Some(ref wt_path), Some(ref wt_branch)) = (&state.active_worktree, &state.active_branch)
    {
        rendered_prompt.push_str(&format!(
            "\n## 🌲 Active Isolated Worktree\n- Worktree Path: {}\n- Branch: {}\n- Working Directory Contract: All code edits, test runs, and git commits MUST be performed inside this worktree!\n",
            wt_path.display(),
            wt_branch
        ));
    }

    // Completed milestones and the epistemic ledger. This is the whole point of
    // recording notes: without replaying them here, each milestone starts blind
    // and `state note` writes to something nothing ever reads. Carrying findings
    // forward -- rather than the whole conversation -- is what keeps the context
    // footprint flat while preserving what was learned.
    if !state.completed_tasks.is_empty() {
        rendered_prompt.push_str("\n## Completed Milestones\n");
        for done in &state.completed_tasks {
            rendered_prompt.push_str(&format!("- {}: {}\n", done.id, done.title));
        }
    }

    let ledger = superstate_core::ledger::read_ledger(state_path);
    if !ledger.is_empty() {
        rendered_prompt.push_str("\n## Epistemic Ledger (findings from earlier milestones)\n");
        for entry in &ledger {
            rendered_prompt.push_str(&format!("- [{}] {}\n", entry.author, entry.note));
        }
    }

    if with_graph {
        let graph = find_graph_context(state_path);
        match graph {
            Some((path, body)) => {
                rendered_prompt.push_str(&format!(
                    "\n## Architecture Context ({})\n{}\n",
                    path.display(),
                    body
                ));
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

    rendered_prompt.push_str("\n## Instructions\n");
    rendered_prompt
        .push_str("1. Implement the requested task following Test-Driven Development (TDD).\n");
    rendered_prompt.push_str("2. Run the targeted verification command to confirm your fix.\n");
    rendered_prompt.push_str("3. Commit your changes with a conventional commit message.\n");
    rendered_prompt.push_str("4. Return a summary with commit SHA and verification output.\n");

    if json_output {
        let json_value = serde_json::json!({
            "task": active,
            "active_worktree": state.active_worktree,
            "active_branch": state.active_branch,
            "prompt": rendered_prompt,
        });
        let serialized = serde_json::to_string_pretty(&json_value)
            .map_err(|e| format!("Failed to serialize prompt JSON: {}", e))?;
        println!("{}", serialized);
    } else {
        print!("{}", rendered_prompt);
    }

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

pub fn cmd_note(state_path: &Path, note: &str, author: &str) -> Result<(), String> {
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
        // Append to the ledger rather than the state's legacy inline array, so
        // there is exactly one place findings are written.
        superstate_core::ledger::append_ledger_note(
            state_path,
            superstate_core::models::LedgerEntry::new("agent", note),
        )
        .map_err(|e| format!("Failed to record note: {}", e))?;
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

    let target_dir = state
        .active_worktree
        .as_ref()
        .filter(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| root.clone());

    let gate = superstate_core::verifier::run_verification_cmd(verify_cmd, &target_dir, 600)
        .map_err(|e| format!("Verification could not run: {}", e))?;

    let mut git_clean_passed = true;
    if gate.passed && verify_git_clean {
        let clean = superstate_core::verifier::verify_git_clean(&target_dir)
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
            if let Some(ref wt_path) = failed_state.active_worktree {
                if wt_path.exists() {
                    let base_branch = WorktreeManager::new(&root)
                        .map(|m| m.default_branch())
                        .unwrap_or_else(|_| "main".to_string());
                    let branch = failed_state
                        .active_branch
                        .clone()
                        .unwrap_or_else(|| {
                            format!(
                                "superstate/{}",
                                superstate_worktree::manager::sanitize_slug(
                                    &failed_state
                                        .active_task
                                        .as_ref()
                                        .map(|t| t.id.as_str())
                                        .unwrap_or("task")
                                )
                            )
                        });
                    let wt = IsolatedWorktree {
                        branch,
                        path: wt_path.clone(),
                        base_branch,
                        repo_root: root.clone(),
                    };
                    let _ = wt.abort();
                    failed_state.active_worktree = None;
                    failed_state.active_branch = None;
                    let _ = save_state_atomic(state_path, &failed_state);
                }
            }
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
