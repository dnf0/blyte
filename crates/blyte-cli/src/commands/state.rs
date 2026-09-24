use crate::state_cli::InitArgs;
use blyte_core::ledger::append_ledger_note;
use blyte_core::models::{ExecutionStatus, LedgerEntry, MilestoneSpec};
use blyte_core::plan_parser::init_state_from_plan;
use blyte_core::prompt::{render_turnkey_prompt, select_task};
use blyte_core::state_machine::{
    advance_step, branch_scoped_state_path, find_branch_state_for_worktree, init_state_direct,
    init_state_milestones, load_state, resolve_state_path, save_state_atomic, STATE_DIR,
};
use blyte_core::verifier::execute_all_gates_filtered;
pub use blyte_worktree::manager::ensure_git_exclude;
use blyte_worktree::manager::{ensure_git_baseline, IsolatedWorktree, WorktreeManager};
use std::path::{Path, PathBuf};

/// Summary representation for listing branch execution states.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct BranchStateSummary {
    pub branch: String,
    pub status: ExecutionStatus,
    pub active_task: Option<String>,
    pub step: usize,
    pub total_steps: usize,
    pub base_branch: Option<String>,
    pub state_file: PathBuf,
}

/// Refuse to create a second state inside a project that already has one.
///
/// Two states for one project diverge silently: one records the work, the other
/// is consulted, and neither is obviously wrong when read on its own.
fn reject_nested_state(state_path: &Path) -> Result<(), String> {
    if state_path.is_absolute() || state_path.exists() {
        return Ok(());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match blyte_core::state_machine::enclosing_state_root(&cwd) {
        Some(root) if root != cwd => Err(format!(
            "This project already has state at {}. Creating another here would give \
             one project two diverging states. Run from {} to reinitialise it, or \
             pass an explicit --state path if a second one is genuinely intended.",
            root.join(blyte_core::state_machine::STATE_DIR)
                .join("state.json")
                .display(),
            root.display(),
        )),
        _ => Ok(()),
    }
}

/// Resolve the project root for a given state path.
///
/// Walks upward through directory components of `state_path` until finding `STATE_DIR`
/// (`.agent-state`), returning its parent as the project root. If `STATE_DIR` is not found,
/// falls back to the state path's parent or current working directory.
pub fn resolve_project_root(state_path: &Path) -> PathBuf {
    blyte_core::resolve_project_root(state_path)
}

/// Dynamically resolve effective state path based on branch-scoped isolation rules.
///
/// 1. If `state_arg` is explicitly provided (not default `.agent-state/state.json`),
///    resolves via standard ancestor search.
/// 2. If inside a worktree, searches ancestors for `.agent-state/branches/` and calls
///    `find_branch_state_for_worktree`.
/// 3. If in a git repository on a feature branch (not `main`, `master`, and not starting
///    with `wt/` or `blyte/`), returns `branch_scoped_state_path(&repo_root, &branch)`.
/// 4. Fallback: returns standard ancestor resolution of `state_arg`.
pub fn resolve_effective_state_path(state_arg: &Path, cwd: &Path) -> PathBuf {
    let default_path = Path::new(crate::state_cli::DEFAULT_STATE);
    if state_arg != default_path && state_arg.as_os_str() != crate::state_cli::DEFAULT_STATE {
        return resolve_state_path(state_arg, cwd);
    }

    // 2. Check if cwd is inside an active worktree
    let wt_branch = WorktreeManager::new(cwd)
        .ok()
        .and_then(|m| m.current_branch().ok());

    let mut ancestor = Some(cwd);
    while let Some(current) = ancestor {
        let branches_dir = current.join(STATE_DIR).join("branches");
        if branches_dir.is_dir() {
            let mut wt_cand = Some(cwd);
            while let Some(p) = wt_cand {
                if p == current {
                    break;
                }
                if let Some(matched) =
                    find_branch_state_for_worktree(current, p, wt_branch.as_deref())
                {
                    return matched;
                }
                wt_cand = p.parent();
            }
        }
        ancestor = current.parent();
    }

    // 3. Check if cwd is in a repository on a feature branch
    if let Ok(manager) = WorktreeManager::new(cwd) {
        if let Ok(branch) = manager.current_branch() {
            let is_feature_branch = branch != "main"
                && branch != "master"
                && !branch.starts_with("wt/")
                && !branch.starts_with("blyte/");
            if is_feature_branch {
                let repo_root = blyte_core::state_machine::enclosing_state_root(cwd)
                    .or_else(|| {
                        let mut d = Some(cwd);
                        while let Some(cur) = d {
                            if cur.join(".git").exists() {
                                return Some(cur.to_path_buf());
                            }
                            d = cur.parent();
                        }
                        None
                    })
                    .unwrap_or_else(|| cwd.to_path_buf());
                let candidate = branch_scoped_state_path(&repo_root, &branch);
                if candidate.exists() {
                    return candidate;
                }
                let default_state = repo_root.join(STATE_DIR).join("state.json");
                if default_state.exists() {
                    return default_state;
                }
                return candidate;
            }
        }
    }

    // 4. Fallback to default state path resolution
    resolve_state_path(state_arg, cwd)
}

/// List active and completed branch-scoped states.
pub fn cmd_list(state_arg: Option<&Path>, as_json: bool, cwd: &Path) -> Result<(), String> {
    let repo_root = if let Some(arg) = state_arg {
        let p = resolve_state_path(arg, cwd);
        if p.is_dir() {
            if p.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
                p.parent().unwrap_or(&p).to_path_buf()
            } else if p.file_name().and_then(|s| s.to_str()) == Some("branches")
                && p.parent()
                    .and_then(|pp| pp.file_name())
                    .and_then(|s| s.to_str())
                    == Some(STATE_DIR)
            {
                p.parent()
                    .and_then(|pp| pp.parent())
                    .unwrap_or(&p)
                    .to_path_buf()
            } else {
                p
            }
        } else {
            resolve_project_root(&p)
        }
    } else {
        blyte_core::state_machine::enclosing_state_root(cwd)
            .or_else(|| {
                let mut d = Some(cwd);
                while let Some(cur) = d {
                    if cur.join(".git").exists() {
                        return Some(cur.to_path_buf());
                    }
                    d = cur.parent();
                }
                None
            })
            .unwrap_or_else(|| cwd.to_path_buf())
    };

    let state_dir = repo_root.join(STATE_DIR);
    let mut summaries = Vec::new();

    // 1. Scan .agent-state/branches/*.json
    let branches_dir = state_dir.join("branches");
    if branches_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&branches_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Ok(state) = load_state(&path) {
                        let slug = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let total_steps = state.completed_tasks.len()
                            + if state.active_task.is_some() { 1 } else { 0 }
                            + state.pending_tasks.len();
                        let active_task = state
                            .active_task
                            .as_ref()
                            .map(|t| format!("{}: {}", t.id, t.title));
                        let display_step = if state.status == ExecutionStatus::Completed {
                            total_steps
                        } else {
                            state.step.min(total_steps)
                        };
                        summaries.push(BranchStateSummary {
                            branch: slug,
                            status: state.status,
                            active_task,
                            step: display_step,
                            total_steps,
                            base_branch: state.base_branch,
                            state_file: path,
                        });
                    }
                }
            }
        }
    }

    // 2. Also check .agent-state/state.json if present
    let default_state_file = state_dir.join("state.json");
    if default_state_file.is_file() {
        if let Ok(state) = load_state(&default_state_file) {
            let total_steps = state.completed_tasks.len()
                + if state.active_task.is_some() { 1 } else { 0 }
                + state.pending_tasks.len();
            let active_task = state
                .active_task
                .as_ref()
                .map(|t| format!("{}: {}", t.id, t.title));
            let default_branch_name = WorktreeManager::new(&repo_root)
                .map(|m| m.default_branch())
                .unwrap_or_else(|_| "main".to_string());
            let branch_display = format!("{} (default)", default_branch_name);
            let display_step = if state.status == ExecutionStatus::Completed {
                total_steps
            } else {
                state.step.min(total_steps)
            };
            summaries.push(BranchStateSummary {
                branch: branch_display,
                status: state.status,
                active_task,
                step: display_step,
                total_steps,
                base_branch: state.base_branch.or(Some(default_branch_name)),
                state_file: default_state_file,
            });
        }
    }

    summaries.sort_by(|a, b| a.branch.cmp(&b.branch));

    if as_json {
        let serialized = serde_json::to_string_pretty(&summaries)
            .map_err(|e| format!("Failed to serialize summaries: {}", e))?;
        println!("{}", serialized);
    } else {
        if summaries.is_empty() {
            println!("No branch execution states found.");
            return Ok(());
        }
        println!("Active & Completed Branches:");
        println!(
            "  {:<26} {:<12} {:<24} {:<6} {:<20}",
            "Branch", "Status", "Active Task", "Step", "Base Branch"
        );
        for s in &summaries {
            let status_str = match s.status {
                ExecutionStatus::NotStarted => "NotStarted",
                ExecutionStatus::InProgress => "InProgress",
                ExecutionStatus::Completed => "Completed",
                ExecutionStatus::Failed => "Failed",
                ExecutionStatus::Blocked => "Blocked",
            };
            let task_str = s.active_task.as_deref().unwrap_or("-");
            let step_str = format!("{}/{}", s.step, s.total_steps);
            let base_str = s.base_branch.as_deref().unwrap_or("-");
            println!(
                "  {:<26} {:<12} {:<24} {:<6} {:<20}",
                s.branch, status_str, task_str, step_str, base_str
            );
        }
    }

    Ok(())
}

/// Initialize project execution state either from plan markdown, directly from a goal, or from milestones.
///
/// # Arguments
/// * `args` - Configuration arguments containing optional plan path, goal, verify_cmd, files, milestones, state path, and no_worktree flag.
///
/// # Errors
/// Returns an error if:
/// * None of `--plan`, `--goal`, or `--milestones` is provided.
/// * More than one of `--plan`, `--goal`, or `--milestones` is provided.
/// * `--goal` is provided without `--verify-cmd`.
/// * Nested state exists within the repository hierarchy.
/// * State saving or worktree provisioning fails.
pub fn cmd_init(args: &InitArgs) -> Result<(), String> {
    // Validate mutually exclusive or required initialization modes
    let modes_count = [
        args.plan.is_some(),
        args.goal.is_some(),
        args.milestones.is_some(),
    ]
    .iter()
    .filter(|&&b| b)
    .count();

    if modes_count == 0 {
        return Err(
            "Must provide either --plan <path> or --goal <text> with --verify-cmd <cmd>, or --milestones <json>"
                .to_string(),
        );
    }
    if modes_count > 1 {
        return Err(
            "Cannot combine --plan, --goal, and --milestones; choose exactly one initialization mode"
                .to_string(),
        );
    }
    if args.goal.is_some() && args.verify_cmd.is_none() {
        return Err("--verify-cmd is required when initializing with --goal".to_string());
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Guard against creating a divergent nested state inside an existing state hierarchy
    reject_nested_state(&args.state)?;
    let effective_state_path = resolve_effective_state_path(&args.state, &cwd);
    let state_path = &effective_state_path;

    // Ensure baseline git repository is present before initializing state
    let root = resolve_project_root(state_path);
    ensure_git_baseline(&root);

    // Detect active feature branch from git repository if available
    let base_branch = if root.join(".git").exists() {
        WorktreeManager::new(&root)
            .ok()
            .and_then(|m| m.current_branch().ok())
    } else {
        None
    };

    let use_worktree = args.use_worktree();
    let no_worktree = !use_worktree;

    // Direct milestones array initialization branch: constructs sequential DAG in-memory
    if let Some(ref raw_milestones) = args.milestones {
        // Read milestone JSON directly from string or piped standard input if '-'
        let raw_json = if raw_milestones.trim() == "-" {
            use std::io::Read;
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|e| format!("Failed to read milestones from stdin: {}", e))?;
            buffer
        } else {
            raw_milestones.clone()
        };

        // Parse JSON array into strongly-typed MilestoneSpec list
        let parsed: Vec<MilestoneSpec> = serde_json::from_str(&raw_json)
            .map_err(|e| format!("Failed to parse milestones JSON: {}", e))?;

        // Initialize multi-milestone DAG with base branch propagation
        let mut state = init_state_milestones(
            args.goal.as_deref(),
            parsed,
            base_branch.as_deref(),
            no_worktree,
        )
        .map_err(|e| match e {
            blyte_core::state_machine::StateError::InvalidTransition(msg) => {
                format!("Failed to initialize state from milestones: {}", msg)
            }
            other => format!("Failed to initialize state from milestones: {}", other),
        })?;

        // Provision isolated worktree for initial active task only if use_worktree is set
        if use_worktree && root.join(".git").exists() {
            ensure_git_exclude(&root);
            let manager = WorktreeManager::new(&root)
                .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
            if let Some(ref active) = state.active_task {
                let wt = manager
                    .ensure_task_worktree(&active.id, base_branch.as_deref())
                    .map_err(|e| {
                        format!("Failed to ensure worktree for task {}: {}", active.id, e)
                    })?;
                state.active_worktree = Some(wt.path.clone());
                state.active_branch = Some(wt.branch.clone());
            }
        }

        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to save state: {}", e))?;

        println!("State initialized at {}", state_path.display());
        if let Some(ref task) = state.active_task {
            println!("Active Task: {}: {}", task.id, task.title);
        }
        return Ok(());
    }

    // Direct state initialization branch: no plan.md is read or created
    if let Some(ref goal) = args.goal {
        let verify_cmd = args.verify_cmd.as_deref().ok_or_else(|| {
            "Missing required verification command (--verify-cmd) for direct state initialization"
                .to_string()
        })?;
        let files_modify: Vec<PathBuf> = args
            .files
            .as_deref()
            .map(|f| {
                f.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();

        let mut state = init_state_direct(goal, verify_cmd, files_modify, no_worktree);
        state.base_branch = base_branch.clone();

        // Provision isolated worktree for the active task only if use_worktree is set
        if use_worktree && root.join(".git").exists() {
            ensure_git_exclude(&root);
            let manager = WorktreeManager::new(&root)
                .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
            if let Some(ref active) = state.active_task {
                let wt = manager
                    .ensure_task_worktree(&active.id, base_branch.as_deref())
                    .map_err(|e| {
                        format!("Failed to ensure worktree for task {}: {}", active.id, e)
                    })?;
                state.active_worktree = Some(wt.path.clone());
                state.active_branch = Some(wt.branch.clone());
            }
        }

        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to save state: {}", e))?;

        println!("State initialized at {}", state_path.display());
        if let Some(ref task) = state.active_task {
            println!("Active Task: {}: {}", task.id, task.title);
        }
        return Ok(());
    }

    // Markdown plan initialization branch: preserves multi-milestone parsing
    if let Some(ref plan) = args.plan {
        let mut state = init_state_from_plan(plan, state_path)
            .map_err(|e| format!("Failed to initialize state from plan: {}", e))?;
        if state.base_branch.is_none() {
            state.base_branch = base_branch.clone();
        }
        state.no_worktree = no_worktree;
        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to save state: {}", e))?;
        println!("State initialized at {}", state_path.display());
        if let Some(ref task) = state.active_task {
            println!("Active Task: {}: {}", task.id, task.title);
        }
        return Ok(());
    }

    Ok(())
}

/// Print tasks whose dependencies are satisfied and may therefore run now.
///
/// `--json` emits an array of titles, which is the form a runner consumes when
/// deciding what to dispatch next.
pub fn cmd_ready(state_path: &Path, as_json: bool) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let ready = blyte_core::state_machine::ready_tasks(&state);

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

/// Tag the current commit as the checkpoint guarding this step.
pub fn cmd_checkpoint(state_path: &Path) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);
    let tag = blyte_core::checkpoint::create_checkpoint(&root, state.step)
        .map_err(|e| format!("Failed to create checkpoint: {}", e))?;
    println!("Checkpoint created: {}", tag);
    Ok(())
}

/// Restore the working tree to a checkpoint, discarding untracked files with it.
pub fn cmd_rollback(state_path: &Path, reference: Option<&str>) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
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
                .unwrap_or_else(|| "wt/task".to_string());
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

    let target =
        blyte_core::checkpoint::rollback_to(&root, state.step, reference, &preserved(state_path))
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
pub fn repo_root_for(state_path: &Path) -> std::path::PathBuf {
    resolve_project_root(state_path)
}

pub fn cmd_status(state_path: &Path, as_json: bool) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let daemon_paths = crate::commands::daemon::daemon_paths_for_state(state_path);
    let pid_file = if daemon_paths.pid_file.exists() {
        daemon_paths.pid_file
    } else {
        state_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("daemon.pid")
    };
    let daemon_info = if pid_file.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                let alive = crate::commands::daemon::is_pid_alive(pid);
                Some((pid, alive))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    if as_json {
        let mut val = serde_json::to_value(&state)
            .map_err(|e| format!("Failed to serialize state: {}", e))?;
        if let Some((pid, alive)) = daemon_info {
            val["daemon"] = serde_json::json!({
                "pid": pid,
                "running": alive,
            });
        }
        let rendered = serde_json::to_string_pretty(&val)
            .map_err(|e| format!("Failed to serialize state JSON: {}", e))?;
        println!("{}", rendered);
        return Ok(());
    }
    println!("State Status: {:?}", state.status);
    println!("Step: {}", state.step);
    if let Some((pid, alive)) = daemon_info {
        if alive {
            println!("Daemon Status: Running (PID: {})", pid);
        } else {
            println!("Daemon Status: Stopped (Stale PID: {})", pid);
        }
    }
    if let Some(ref task) = state.active_task {
        println!("Active Task: {}: {}", task.id, task.title);
    } else {
        println!("Active Task: None");
    }
    println!("Completed Tasks: {}", state.completed_tasks.len());
    println!(
        "Ledger Entries: {}",
        blyte_core::ledger::read_ledger(state_path).len()
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

/// Show execution state as formatted markdown or machine-readable JSON.
///
/// # Arguments
/// * `state_path` - Path to the state JSON file.
/// * `_markdown` - Flag indicating markdown output requested (default behaviour).
/// * `as_json` - Flag indicating JSON output requested.
///
/// # Errors
/// Returns an error if the state file cannot be loaded or serialized.
pub fn cmd_show(state_path: &Path, _markdown: bool, as_json: bool) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    // Machine-readable JSON output requested
    if as_json {
        let rendered = serde_json::to_string_pretty(&state)
            .map_err(|e| format!("Failed to serialize state: {}", e))?;
        println!("{}", rendered);
        return Ok(());
    }

    // Default clean markdown view output
    println!("# Blyte Execution Status");
    println!("- **Status:** {:?} (Step {})", state.status, state.step);
    if let Some(ref base) = state.base_branch {
        println!("- **Base Branch:** `{}`", base);
    }
    if let Some(ref task) = state.active_task {
        println!("- **Active Task:** {}: {}", task.id, task.title);
        if let Some(ref cmd) = task.verification_cmd {
            println!("- **Verification Command:** `{}`", cmd);
        }
    } else {
        println!("- **Active Task:** None");
    }

    // Include active worktree and branch if provisioned
    if let Some(ref wt) = state.active_worktree {
        if let Some(ref branch) = state.active_branch {
            println!(
                "- **Active Worktree:** `{}` (branch `{}`)",
                wt.display(),
                branch
            );
        } else {
            println!("- **Active Worktree:** `{}`", wt.display());
        }
    }

    Ok(())
}

pub fn cmd_next(
    state_path: &Path,
    verify_cmd: Option<String>,
    verify_git_clean: bool,
    ignore_untracked_glob: Option<&str>,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let active_task = match state.active_task.clone() {
        Some(t) => t,
        None => return Err("No active task to advance".to_string()),
    };

    let mut task_to_verify = active_task.clone();
    if let Some(cmd) = verify_cmd {
        task_to_verify.verification_cmd = Some(cmd);
    }

    let repo_root = resolve_project_root(state_path);
    ensure_git_exclude(&repo_root);

    // When an isolated worktree is active, gates must run within that worktree checkout
    // where task changes were staged and committed, rather than the primary repo root.
    let target_dir = state
        .active_worktree
        .as_ref()
        .filter(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| repo_root.clone());

    let results = execute_all_gates_filtered(
        &target_dir,
        &task_to_verify,
        verify_git_clean,
        ignore_untracked_glob,
    )
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
            // Target the recorded feature base branch, falling back to current branch or default
            let base_branch = state.base_branch.clone().unwrap_or_else(|| {
                WorktreeManager::new(&repo_root)
                    .map(|m| m.current_branch().unwrap_or_else(|_| m.default_branch()))
                    .unwrap_or_else(|_| "main".to_string())
            });
            let branch = state.active_branch.clone().unwrap_or_else(|| {
                blyte_worktree::manager::task_worktree_names(
                    &repo_root,
                    &active_task.id,
                    state.base_branch.as_deref(),
                )
                .0
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

    match advance_step(&mut state) {
        Ok(()) => {
            save_state_atomic(state_path, &state)
                .map_err(|e| format!("Failed to save state: {}", e))?;

            // Provision worktree for newly active task in sequential progression
            if !state.no_worktree && repo_root.join(".git").exists() {
                if let Some(ref next_task) = state.active_task {
                    let manager = WorktreeManager::new(&repo_root)
                        .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
                    let wt = manager
                        .ensure_task_worktree(&next_task.id, state.base_branch.as_deref())
                        .map_err(|e| {
                            format!("Failed to ensure worktree for task {}: {}", next_task.id, e)
                        })?;
                    state.active_worktree = Some(wt.path);
                    state.active_branch = Some(wt.branch);
                    save_state_atomic(state_path, &state)
                        .map_err(|e| format!("Failed to save state: {}", e))?;
                }
            }

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
        Err(blyte_core::state_machine::StateError::InvalidTransition(msg)) => {
            // Guard against DAG deadlock: milestone completed successfully, but remaining
            // pending tasks cannot advance due to missing or cyclical dependencies.
            state.status = ExecutionStatus::Blocked;
            state.notes.push(LedgerEntry::new(
                "blyte-daemon",
                format!(
                    "[dag-blocked] Milestone {} completed, but remaining tasks cannot advance: {}",
                    active_task.id, msg
                ),
            ));
            save_state_atomic(state_path, &state)
                .map_err(|e| format!("Failed to save state: {}", e))?;
            println!(
                "State blocked: Milestone {} completed, but remaining tasks cannot advance: {}",
                active_task.id, msg
            );
            Ok(())
        }
        Err(e) => Err(format!("Failed to advance step: {}", e)),
    }
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
    no_worktree: bool,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;

    let is_in_place = no_worktree || state.no_worktree;

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
    if !is_in_place && repo_root.join(".git").exists() {
        ensure_git_exclude(&repo_root);
        let manager = WorktreeManager::new(&repo_root)
            .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
        let wt = manager
            .ensure_task_worktree(&active.id, state.base_branch.as_deref())
            .map_err(|e| format!("Failed to ensure worktree for task {}: {}", active.id, e))?;
        state.active_worktree = Some(wt.path.clone());
        state.active_branch = Some(wt.branch.clone());
        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to save state: {}", e))?;
    } else if is_in_place && (state.active_worktree.is_some() || state.active_branch.is_some()) {
        state.active_worktree = None;
        state.active_branch = None;
        save_state_atomic(state_path, &state)
            .map_err(|e| format!("Failed to save state: {}", e))?;
    }
    let mut rendered_prompt =
        render_turnkey_prompt(&state, state_path, task, is_in_place, with_graph)?;

    if !rendered_prompt.contains("- **State File:**") {
        let state_display = state_path.display();
        let contract_additions = format!(
            "- **State File:** `{}`\n- **Next Gate Command:** `blyte state next --state \"{}\"`\n",
            state_display, state_display
        );
        if let Some(pos) = rendered_prompt.find("## Instructions\n") {
            let insert_idx = pos + "## Instructions\n".len();
            rendered_prompt.insert_str(insert_idx, &contract_additions);
        } else {
            rendered_prompt.push_str(&format!("\n{}", contract_additions));
        }
    }

    if json_output {
        let json_value = serde_json::json!({
            "task": active,
            "active_worktree": if is_in_place { serde_json::Value::Null } else { serde_json::to_value(&state.active_worktree).unwrap_or(serde_json::Value::Null) },
            "active_branch": if is_in_place { serde_json::Value::Null } else { serde_json::to_value(&state.active_branch).unwrap_or(serde_json::Value::Null) },
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

pub fn cmd_note(state_path: &Path, note: &str, author: &str) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let plan = std::fs::read_to_string(plan_path)
        .map_err(|e| format!("Failed to read plan {}: {}", plan_path.display(), e))?;

    let report = blyte_core::maintenance::sync_with_plan(&mut state, &plan)?;
    state.plan_path = Some(plan_path.to_path_buf());
    write_json(
        out.unwrap_or(state_path),
        &blyte_core::patching::state_to_value(&state)?,
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let mut state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let plan = match plan_path {
        Some(p) => {
            Some(std::fs::read_to_string(p).map_err(|e| format!("Failed to read plan: {}", e))?)
        }
        None => None,
    };

    let report = blyte_core::maintenance::diagnose(&state, plan.as_deref());
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
        let actions = blyte_core::maintenance::repair(&mut state);
        if actions.is_empty() {
            println!("Nothing to repair.");
        } else {
            write_json(state_path, &blyte_core::patching::state_to_value(&state)?)?;
            for a in actions {
                println!("  repaired: {}", a);
            }
        }
    }
    Ok(())
}

/// Validate a state document against the execution schema.
pub fn cmd_validate(state_path: &Path) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let raw = read_json(state_path)?;
    let errors = blyte_core::patching::validate_state(&raw);
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    if patch_path.is_none() && note.is_none() {
        return Err("Nothing to apply: pass --patch and/or --note.".to_string());
    }
    let mut state = read_json(state_path)?;

    if let Some(patch_path) = patch_path {
        let patch = read_json(patch_path)?;
        blyte_core::patching::apply_patch(&mut state, &patch);
    }
    if let Some(note) = note {
        // Append to the ledger rather than the state's legacy inline array, so
        // there is exactly one place findings are written.
        blyte_core::ledger::append_ledger_note(
            state_path,
            blyte_core::models::LedgerEntry::new("agent", note),
        )
        .map_err(|e| format!("Failed to record note: {}", e))?;
    }

    // Refuse to write a state we would not accept on read.
    let errors = blyte_core::patching::validate_state(&state);
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
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

    let notes = blyte_core::ledger::read_ledger(state_path);
    println!(
        "{}",
        blyte_core::maintenance::resume_briefing(&state, &notes, &commits)
    );
    Ok(())
}

/// Task telemetry and efficiency metrics.
pub fn cmd_metrics(state_path: &Path, as_json: bool) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let notes = blyte_core::ledger::read_ledger(state_path);
    let m = blyte_core::maintenance::metrics(&state, notes.len());
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
    let decomposed = blyte_core::patching::decompose_plan(&plan)?;

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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let effective = resolve_effective_state_path(state_path, &cwd);
    let state_path = &effective;
    let state = load_state(state_path).map_err(|e| format!("Failed to load state: {}", e))?;
    let root = repo_root_for(state_path);
    let step = state.step;

    if checkpoint {
        blyte_core::checkpoint::create_checkpoint(&root, step)
            .map_err(|e| format!("Failed to checkpoint before exec: {}", e))?;
    }

    let target_dir = state
        .active_worktree
        .as_ref()
        .filter(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| root.clone());

    let gate = blyte_core::verifier::run_verification_cmd(verify_cmd, &target_dir, 600)
        .map_err(|e| format!("Verification could not run: {}", e))?;

    let mut git_clean_passed = true;
    if gate.passed && verify_git_clean {
        let clean = blyte_core::verifier::verify_git_clean(&target_dir)
            .map_err(|e| format!("git clean check failed: {}", e))?;
        git_clean_passed = clean.passed;
    }

    let passed = gate.passed && git_clean_passed;
    let mut rolled_back = false;
    let mut advanced = false;

    if passed && advance {
        cmd_next(
            state_path,
            Some(verify_cmd.to_string()),
            verify_git_clean,
            None,
        )?;
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
        let value = blyte_core::patching::state_to_value(&failed_state)?;
        write_json(state_path, &value)?;

        if rollback_on_fail {
            if let Some(ref wt_path) = failed_state.active_worktree {
                if wt_path.exists() {
                    let base_branch = WorktreeManager::new(&root)
                        .map(|m| m.default_branch())
                        .unwrap_or_else(|_| "main".to_string());
                    let branch = failed_state.active_branch.clone().unwrap_or_else(|| {
                        format!(
                            "wt/{}",
                            blyte_worktree::manager::sanitize_slug(
                                failed_state
                                    .active_task
                                    .as_ref()
                                    .map(|t| t.id.as_str())
                                    .unwrap_or("task"),
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
            blyte_core::checkpoint::rollback_to(&root, step, None, &preserved(state_path))
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
