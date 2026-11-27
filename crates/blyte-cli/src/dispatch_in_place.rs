use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::DispatchArgs;
use crate::dispatch::{
    exit_code, load_task_text, parse_pass_report, pass_report_from_error, quick_steps,
    resolve_max_cost, run_implementer_pass, write_start_record, write_summary_to_dir,
    DispatchSummary, ImplementerConfig,
};

pub const IN_PLACE_RULES: &str = r#"Rules:
- You are working directly in the user's folder, not a copy.
- Change files only with the write and edit tools, because shell commands cannot write this folder.
- There is no git here, so do not commit.
- Change only what the task needs.
- Finish with a short report of what you changed."#;

pub fn wrap_in_place_task(task: &str) -> String {
    format!(
        "You are working directly in the user's folder, not a copy.\n\n{}\n\n{}",
        task.trim(),
        IN_PLACE_RULES
    )
}

pub fn records_dir_for(cwd: &Path) -> Result<PathBuf, String> {
    let canonical = cwd
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", cwd.display()))?;
    let out = Command::new("git")
        .current_dir(&canonical)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ])
        .output()
        .map_err(|e| format!("git not found: {e}"))?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let root = PathBuf::from(
            lines
                .next()
                .ok_or_else(|| "git rev-parse returned no output".to_string())?
                .trim(),
        );
        let git_dir = lines.next().map(|s| PathBuf::from(s.trim()));
        let common_dir = lines.next().map(|s| PathBuf::from(s.trim()));
        let main_root = match (git_dir, common_dir) {
            (Some(g), Some(c)) if g != c => {
                let main = (|| {
                    if c.file_name().and_then(|n| n.to_str()) != Some(".git") {
                        return None;
                    }
                    let parent = c.parent()?;
                    let parent_canon = parent.canonicalize().ok()?;
                    let root_canon = root.canonicalize().ok()?;
                    if parent_canon == root_canon {
                        return None;
                    }
                    Some(parent.to_path_buf())
                })();
                main.ok_or_else(|| {
                    format!(
                        "cannot find main checkout for linked worktree {}",
                        root.display()
                    )
                })?
            }
            _ => root,
        };
        return Ok(main_root.join(".wt"));
    }
    let state_dir = crate::surface_registry::state_dir()?;
    crate::surface_dir::run_store(&state_dir, &canonical)
}

pub fn in_place_outcome(
    report_outcome: &str,
    has_gate: bool,
    has_changes: bool,
) -> (&'static str, Option<String>) {
    match (report_outcome, has_gate) {
        ("finished", false) => {
            if has_changes {
                ("finished", None)
            } else {
                ("no_changes", None)
            }
        }
        ("verified", true) => {
            if has_changes {
                ("verified", None)
            } else {
                ("no_changes", None)
            }
        }
        ("gate_failed", _) => ("gate_failed", None),
        ("budget_exhausted", _) => ("budget_exhausted", None),
        ("error", _) => ("error", None),
        (other, _) => (
            "error",
            Some(format!("unexpected child report outcome: {other}")),
        ),
    }
}

pub fn unix_timestamp_secs(now: std::time::SystemTime) -> Result<u64, String> {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| format!("system clock is before UNIX_EPOCH: {e}"))
}

struct FreshStartGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for FreshStartGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn run(args: &DispatchArgs) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let root = match cwd.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot resolve {}: {e}", cwd.display());
            return 2;
        }
    };

    if args.research {
        eprintln!(
            "error: research runs in a folder that is not a git repository are not supported yet"
        );
        return 2;
    }
    if args.base != "HEAD" {
        eprintln!("error: --base needs a git repository");
        return 2;
    }

    let state_dir = match crate::surface_registry::state_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let store = match crate::surface_dir::run_store(&state_dir, &root) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let lock_path = match crate::surface_dir::in_place_lock_path(&state_dir, &root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let task_md_path = store.join(format!("{}.task.md", args.id));
    let start_path = store.join(format!("{}.start.json", args.id));
    let log_path = store.join(format!("{}.log", args.id));
    let report_path = store.join(format!("{}.report.json", args.id));
    let summary_path = store.join(format!("{}.json", args.id));
    let manifest_path = store.join(format!("{}.changes.json", args.id));
    let diff_path = store.join(format!("{}.diff", args.id));
    let before_dir = store.join(format!("{}.before", args.id));

    if summary_path.exists() {
        eprintln!(
            "error: run {} has already finished here; use a new id",
            args.id
        );
        return 2;
    }
    let is_restart = start_path.exists();

    let holder = blyte_agent::folder_lock::Holder {
        pid: std::process::id(),
        kind: "run".to_string(),
        id: args.id.clone(),
    };
    let _lock = match blyte_agent::folder_lock::acquire(&lock_path, &holder) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let repo_cfg = match crate::config::load(&root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let dispatch_cfg = repo_cfg.map(|c| c.dispatch).unwrap_or_default();

    let (gate, gate_source) = match (&args.gate, &dispatch_cfg.gate) {
        (Some(g), _) if !g.trim().is_empty() => (g.clone(), "flag"),
        (Some(_), _) => (String::new(), "none"),
        (None, Some(g)) if !g.trim().is_empty() => (g.clone(), "blyte.toml"),
        _ => (String::new(), "none"),
    };
    let has_gate = !gate.trim().is_empty();

    let (task_content, recorded_task_file) = match load_task_text(
        args.task_file.as_deref(),
        args.plan.as_deref(),
        args.plan_task.as_deref(),
    ) {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            eprintln!("error: give a task file, or --plan with --plan-task");
            return 2;
        }
        Err(msg) => {
            if msg.starts_with("error:") {
                eprintln!("{msg}");
            } else {
                eprintln!("error: {msg}");
            }
            return 2;
        }
    };

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config_allow_read =
        match crate::config::expand_allow_read(&dispatch_cfg.allow_read, home.as_deref(), &root) {
            Ok(paths) => paths,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot find blyte binary: {e}");
            return 2;
        }
    };

    let model = args.model.as_deref().or(dispatch_cfg.model.as_deref());
    let max_seconds = args
        .max_seconds
        .or(dispatch_cfg.max_seconds)
        .unwrap_or(3600);
    let max_turns = args.max_turns.or(dispatch_cfg.max_turns).unwrap_or(300);
    let max_cost = resolve_max_cost(args.max_cost_usd, dispatch_cfg.max_cost);
    let fold_budget = args.fold_budget.or(dispatch_cfg.fold_budget);
    let network = args
        .network
        .as_deref()
        .or(dispatch_cfg.network.as_deref())
        .unwrap_or("on");

    if let Err(e) = std::fs::write(&task_md_path, &task_content) {
        eprintln!("error: could not write {}: {e}", task_md_path.display());
        return 2;
    }

    let mut prompt = wrap_in_place_task(&task_content);
    if has_gate {
        if let Some(qs) = quick_steps(&gate) {
            prompt.push_str("\n\n");
            prompt.push_str(&qs);
        }
    }

    let started_at = match unix_timestamp_secs(std::time::SystemTime::now()) {
        Ok(secs) => secs,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let env_names: Vec<&String> = dispatch_cfg.env.keys().collect();
    let session = crate::usage_log::session_from(
        std::env::var("BLYTE_SESSION").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    );
    let mut start_record = serde_json::json!({
        "id": args.id,
        "pid": std::process::id(),
        "started_at": started_at,
        "task_file": recorded_task_file,
        "gate": gate,
        "tags": args.tags,
        "env": env_names,
        "max_turns": max_turns,
        "max_seconds": max_seconds,
        "plan": args.plan,
        "plan_task": args.plan_task,
        "request": args.request,
        "surface": args.surface,
        "base": serde_json::Value::Null,
        "base_sha": serde_json::Value::Null,
        "base_check": "off",
        "comments": "off",
        "in_place": true,
        "worktree": root.display().to_string(),
        "branch": "",
    });
    if let Some(ref started_by) = args.started_by {
        start_record["started_by"] = serde_json::json!(started_by);
    }
    if let Some(ref s) = session {
        start_record["session"] = serde_json::json!(s);
    }

    if let Err(e) = write_start_record(&start_path, &start_record) {
        eprintln!(
            "error: could not write the start record {}: {e}",
            start_path.display()
        );
        return 2;
    }
    let mut start_guard = FreshStartGuard {
        path: start_path.clone(),
        armed: !is_restart,
    };

    let impl_cfg = ImplementerConfig {
        current_exe: &current_exe,
        worktree_dir: &root,
        root: &root,
        id: &args.id,
        gate: &gate,
        env: &dispatch_cfg.env,
        model,
        max_seconds,
        max_turns,
        max_cost,
        fold_budget,
        sandbox: &args.sandbox,
        network,
        config_allow_read: &config_allow_read,
        cli_allow_read: &args.allow_read,
        pass_env: &args.pass_env,
        research: false,
        no_claim_check: args.no_claim_check,
        left_out: &[],
        snapshot_dir: Some(&before_dir),
        snapshot_manifest: Some(&manifest_path),
    };

    let append_log_header = is_restart.then_some("== restart");
    let mut spawned = false;
    let run_res = run_implementer_pass(
        &impl_cfg,
        &log_path,
        append_log_header,
        &report_path,
        &prompt,
        None,
        |child_pid| {
            spawned = true;
            start_guard.armed = false;
            start_record["pid"] = serde_json::json!(child_pid);
            let _ = write_start_record(&start_path, &start_record);
        },
    );

    let pass = match run_res {
        Ok(()) => match parse_pass_report(&report_path, &args.sandbox) {
            Ok(r) => r,
            Err(err_msg) => pass_report_from_error(err_msg, &args.sandbox),
        },
        Err(err_msg) if !spawned => {
            if err_msg.starts_with("error") {
                eprintln!("{err_msg}");
            } else {
                eprintln!("error: {err_msg}");
            }
            return 2;
        }
        Err(err_msg) => pass_report_from_error(err_msg, &args.sandbox),
    };

    let snapshot_result =
        blyte_agent::snapshot_fs::changed_files(&root, &before_dir, &manifest_path).and_then(
            |files| {
                let diff =
                    blyte_agent::snapshot_fs::unified_diff(&root, &before_dir, &manifest_path)?;
                std::fs::write(&diff_path, &diff)
                    .map_err(|e| format!("cannot write {}: {e}", diff_path.display()))?;
                Ok(files)
            },
        );

    let (changed_files, outcome, error) = match snapshot_result {
        Ok(files) => {
            let (outcome, unexpected_err) =
                in_place_outcome(&pass.report_outcome, has_gate, !files.is_empty());
            let error = unexpected_err.or(pass.error);
            (files, outcome, error)
        }
        Err(err_msg) => (Vec::new(), "error", Some(err_msg)),
    };

    let summary = DispatchSummary {
        id: args.id.clone(),
        worktree: root.display().to_string(),
        branch: String::new(),
        outcome,
        exit: exit_code(outcome),
        error,
        commits: Vec::new(),
        changed_files,
        final_message: pass.final_message,
        turns: pass.turns,
        gate_attempts: pass.gate_attempts,
        gate_failures: pass.gate_failures,
        cost_usd: pass.cost_usd,
        model: pass.model,
        seconds: pass.seconds,
        sandbox: pass.sandbox,
        log: log_path.display().to_string(),
        report: report_path.display().to_string(),
        gate_source,
        research_report: None,
        base_check_seconds: None,
        warnings: Vec::new(),
        fix_round: None,
    };

    let (_, exit) = write_summary_to_dir(
        &summary,
        &store,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    drop(_lock);
    exit
}
