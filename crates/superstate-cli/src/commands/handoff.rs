//! Implementation of the `superstate state handoff` subcommand.
//!
//! Prepares standardized cross-agent handoff packages for seamless transitions
//! between Claude Code, Google Antigravity (AGY), and autonomous Superstate daemons.
//! Coordinates state synchronization, optional Git worktree provisioning,
//! epistemic ledger recording, and background daemon dispatch.

use std::fs;
use std::path::PathBuf;

use superstate_core::daemon::resolve_driver_command;
use superstate_core::handoff::{generate_resumption_command, write_handoff_package, HandoffSpec};
use superstate_core::ledger::{append_ledger_note, read_ledger};
use superstate_core::models::LedgerEntry;
use superstate_core::plan_parser::init_state_from_plan;
use superstate_core::state_machine::{
    init_state_direct, init_state_milestones, load_state, sanitize_branch_slug, save_state_atomic,
};
use superstate_core::tdd::tdd_milestones;
use superstate_worktree::manager::{ensure_git_baseline, ensure_git_exclude, WorktreeManager};

use crate::commands::daemon::cmd_daemon;
use crate::commands::state::{repo_root_for, resolve_effective_state_path};
use crate::state_cli::HandoffArgs;

/// Prepares and outputs a standardized cross-agent handoff package.
///
/// # Arguments
/// * `args` - Configuration arguments controlling state resolution, source/target agents,
///   worktree isolation, detached background execution, and driver selection.
///
/// # Returns
/// `Ok(())` on successful handoff package generation and optional daemon dispatch.
///
/// # Errors
/// Returns an error string if state loading, git operations, file persistence, or daemon spawning fails.
pub fn cmd_handoff(args: &HandoffArgs) -> Result<(), String> {
    // Resolve effective state machine path and project repository root
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let state_path = resolve_effective_state_path(&args.state, &cwd);
    let repo_root = repo_root_for(&state_path);

    // Ensure git repository baseline exists before evaluating branches and worktrees
    ensure_git_baseline(&repo_root);

    let use_worktree = args.use_worktree();
    let no_worktree = !use_worktree;

    // Detect base branch from active git repository to support merge back workflows
    let base_branch = if repo_root.join(".git").exists() {
        WorktreeManager::new(&repo_root)
            .ok()
            .and_then(|m| m.current_branch().ok())
    } else {
        None
    };

    // Load existing execution state or initialize a new state machine on-demand
    let mut state = if state_path.is_file() {
        load_state(&state_path).map_err(|e| format!("Failed to load state: {}", e))?
    } else if let Some(ref plan_path) = args.plan {
        // Plan-driven state initialization: parses milestones and initializes DAG
        let mut st = init_state_from_plan(plan_path, &state_path)
            .map_err(|e| format!("Failed to initialize state from plan: {}", e))?;
        if st.base_branch.is_none() {
            st.base_branch = base_branch.clone();
        }
        st.no_worktree = no_worktree;
        st
    } else if args.tdd {
        // Autonomous TDD decomposition. The DAG is derived here, inside the binary, from the
        // repository's own layout -- never supplied by a caller. Manifesto Invariant V: a
        // hand-authored gate turns open-ended diagnosis into a narrow test-passing game, so
        // the worker must author the reproduction test itself and the control plane only
        // certifies that it is RED and later GREEN.
        let goal = args.goal.as_deref().ok_or_else(|| {
            "--tdd requires --goal <text> describing the issue to reproduce".to_string()
        })?;
        let milestones = tdd_milestones(&repo_root).map_err(|e| e.to_string())?;
        let mut st = init_state_milestones(Some(goal), milestones, base_branch.as_deref(), no_worktree)
            .map_err(|e| format!("Failed to initialize TDD state: {}", e))?;
        st.base_branch = base_branch.clone();
        st.no_worktree = no_worktree;
        st
    } else if let Some(ref goal) = args.goal {
        // Direct goal-driven state initialization: constructs initial milestone task
        let verify_cmd = args.verify_cmd.as_deref().unwrap_or("true");
        let mut st = init_state_direct(goal, verify_cmd, Vec::new(), no_worktree);
        st.base_branch = base_branch.clone();
        if let Some(ref mut task) = st.active_task {
            task.verification_cmd = args.verify_cmd.clone();
        }
        st
    } else {
        return Err(
            "State does not exist. Provide --goal <text> or --plan <path> to initialize handoff."
                .to_string(),
        );
    };

    // Ensure state directory hierarchy exists before persisting updated state
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create state directory: {}", e))?;
    }

    // Provision isolated worktree for the active task when enabled and git is present
    if use_worktree && repo_root.join(".git").exists() {
        ensure_git_exclude(&repo_root);
        let manager = WorktreeManager::new(&repo_root)
            .map_err(|e| format!("Failed to initialize WorktreeManager: {}", e))?;
        if let Some(ref active) = state.active_task {
            if state.active_worktree.is_none() {
                let wt = manager
                    .ensure_task_worktree(&active.id, state.base_branch.as_deref())
                    .map_err(|e| {
                        format!("Failed to ensure worktree for task {}: {}", active.id, e)
                    })?;
                state.active_worktree = Some(wt.path.clone());
                state.active_branch = Some(wt.branch.clone());
            }
        }
    }

    // Persist current state machine configuration atomically
    save_state_atomic(&state_path, &state).map_err(|e| format!("Failed to save state: {}", e))?;

    // Determine source agent from explicit flag or ambient runtime environment variables
    let from_agent = if let Some(ref from) = args.from {
        from.clone()
    } else if std::env::var("CLAUDE_PROJECT_ROOT").is_ok() {
        "claude".to_string()
    } else if std::env::var("AGY_VERSION").is_ok() {
        "agy".to_string()
    } else {
        "human".to_string()
    };

    // Determine target agent preset or generic fallback
    let to_agent = args.to.clone().unwrap_or_else(|| "any".to_string());

    // Expand driver preset flags (e.g. claude -> claude -p --dangerously-skip-permissions)
    let resolved_driver = args.driver.as_deref().map(resolve_driver_command);

    // Derive deterministic job slug for file naming and process coordination
    let (default_slug, _) = superstate_core::daemon::resolve_daemon_slug_and_state_dir(&state_path);
    let slug = if default_slug != "default" {
        default_slug
    } else if let Some(ref b) = state.active_branch {
        sanitize_branch_slug(b)
    } else if let Some(ref t) = state.active_task {
        sanitize_branch_slug(&t.id)
    } else {
        "default".to_string()
    };

    // Assemble handoff specification and load epistemic ledger history
    let spec = HandoffSpec {
        from_agent: Some(from_agent.clone()),
        to_agent: Some(to_agent.clone()),
        state_path: state_path.clone(),
        worktree_path: state.active_worktree.clone(),
        slug: slug.clone(),
    };

    let notes = read_ledger(&state_path);
    let handoff_file = write_handoff_package(&repo_root, &state, &spec, &notes)
        .map_err(|e| format!("Failed to write handoff package: {}", e))?;

    // Record [handoff] metadata note in epistemic ledger for provenance tracking
    let active_task_desc = state
        .active_task
        .as_ref()
        .map(|t| format!("{}: {}", t.id, t.title))
        .unwrap_or_else(|| "none".to_string());
    let handoff_note = format!(
        "[handoff] Handed off to {}: active task {}",
        to_agent, active_task_desc
    );
    let _ = append_ledger_note(&state_path, LedgerEntry::new(&from_agent, handoff_note));

    // Generate formatted copy-pasteable target resumption command
    let target_resumption_cmd =
        generate_resumption_command(&to_agent, state.active_worktree.as_deref(), &state_path);

    // Route execution based on invocation mode: detached daemon, JSON output, or interactive text
    if args.detach {
        let driver_str = resolved_driver.as_deref().or(args.driver.as_deref());
        cmd_daemon(
            &state_path,
            driver_str,
            true,
            3,
            300,
            false,
            false,
            false,
            superstate_core::daemon::RollbackPolicy::default(),
            true,
            true,
            None, // read_threshold_lines: take the measured default / env override
            true,
            false,
        )
    } else if args.json {
        let active_task_str = state
            .active_task
            .as_ref()
            .map(|t| format!("{}: {}", t.id, t.title));
        let payload = serde_json::json!({
            "status": "ready",
            "slug": slug,
            "active_task": active_task_str,
            "worktree_path": state.active_worktree.as_ref().map(|p| p.display().to_string()),
            "handoff_file": handoff_file.display().to_string(),
            "target_resumption_command": target_resumption_cmd,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        Ok(())
    } else {
        println!("🤝 Handoff package written to {}", handoff_file.display());
        println!("- Source Agent: {}", from_agent);
        println!("- Target Agent: {}", to_agent);
        println!("- Slug: {}", slug);
        if let Some(ref t) = state.active_task {
            println!("- Active Task: {}: {}", t.id, t.title);
        }
        if let Some(ref wt) = state.active_worktree {
            println!("- Worktree: {}", wt.display());
        }
        println!("\n🚀 Resumption Command:");
        println!("  {}", target_resumption_cmd);
        Ok(())
    }
}
