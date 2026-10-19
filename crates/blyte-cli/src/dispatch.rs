//! `blyte dispatch` and `blyte review` commands.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{DispatchArgs, ReviewArgs};

pub const DISPATCH_RULES: &str = r#"Rules:
- Stay within the task. If it cannot pass without a small change elsewhere, make the smallest one and say so.
- Never delete, clean or modify anything outside the worktree, and never change the machine to make a check pass. If a check fails because of the environment (disk, network, a missing service), stop and say so.
- Find files with the glob tool and search file contents with the grep tool. Do not run `find`, `grep -r` or `rg` in the shell over `/`, your home directory, or any directory outside the worktree: they walk huge trees and are killed at the time limit.
- Do not push, open pull requests, or read secrets.
- Commit locally before you stop, with the exact commit message the task gives (or a conventional one if it gives none).
- When you believe the task is done and committed, stop calling tools: the harness then runs the check. Before that, run the check's quick steps (formatting, lint, the tests for your change) yourself; the full run happens at the gate.
- Do not run the full test suite or the whole check yourself, not before you start and not while you work: the gate runs it when you stop, and each extra run costs minutes. Run the tests for the code you change.
- Finish with a short report: what you changed, what you ran, and anything that deviated from the task."#;

pub fn wrap_task(task: &str) -> String {
    format!(
        "You are implementing exactly one task. Your working directory is a git worktree on its own branch; work only there.\n\n{}\n\n{}",
        task.trim(),
        DISPATCH_RULES
    )
}

pub const RESEARCH_RULES: &str = r#"Rules:
- Do not create, modify, move or delete any file anywhere except `report.md` in your working directory.
- Do not run git commands that change state (no checkout, pull, commit, stash, worktree), and do not build, install, push, open pull requests, or write to any service.
- Read the code with the read, grep and glob tools, using absolute paths under the repository (glob and grep with `path` set to the repository's absolute path); use the shell only when they cannot do what you need. Do not run `find`, `grep -r` or `rg` in the shell over `/`, your home directory, or any directory outside the repository: they walk huge trees and are killed at the time limit.
- Answer the question with evidence: file paths with line numbers, and quote only what is needed.
- Quote code only from what you have just read. Earlier tool output is folded out of your context as the run goes on; read the file again before quoting it, never quote from memory.
- Write your findings to `report.md` in your working directory, then stop calling tools and reply with one line summarising them.
- The harness checks report.md: every cited `path:line` must exist, and a code block must be copied from the file and lines cited on the line before it (`path:N-M`). A code block with no citation fails unless it is a command or output (```bash, ```text) or is tagged ```proposed (code you propose or sketch, not in the repository). If the check fails you get the list of problems to fix.
- Start report.md early, after your first findings, and keep it updated as you go, so it always holds your best answer."#;

pub fn wrap_research(question: &str, root: &Path) -> String {
    format!(
        "READ-ONLY RESEARCH TASK. The code to research is under {}: read it there by absolute path.\n\n{}\n\n{}",
        root.display(),
        question.trim(),
        RESEARCH_RULES
    )
}

pub fn quick_steps(gate: &str) -> Option<String> {
    let lines: Vec<&str> = gate
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let steps = lines[..lines.len() - 1]
        .iter()
        .map(|s| format!("`{s}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "The check's quick steps, to run yourself before you stop: {steps}. The last step, the full run, happens at the gate."
    ))
}

pub fn dispatch_outcome(report_outcome: &str, commits: usize) -> &'static str {
    match report_outcome {
        "verified" => {
            if commits == 0 {
                "no_changes"
            } else {
                "verified"
            }
        }
        "gate_failed" => "gate_failed",
        "budget_exhausted" => "budget_exhausted",
        "blocked" => "blocked",
        "error" => "error",
        _ => "error",
    }
}

pub fn exit_code(outcome: &str) -> i32 {
    match outcome {
        "verified" => 0,
        "no_changes" => 4,
        "blocked" => 5,
        _ => 1,
    }
}

#[derive(Debug, Serialize)]
pub struct CommitRecord {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Serialize)]
pub struct DispatchSummary {
    pub id: String,
    pub worktree: String,
    pub branch: String,
    pub outcome: &'static str,
    pub exit: i32,
    pub error: Option<String>,
    pub commits: Vec<CommitRecord>,
    pub changed_files: Vec<String>,
    pub final_message: Option<String>,
    pub turns: u32,
    pub gate_attempts: u32,
    pub gate_failures: Vec<String>,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
    pub seconds: u64,
    pub sandbox: String,
    pub log: String,
    pub report: String,
    pub gate_source: &'static str,
    pub research_report: Option<String>,
}

fn finish_summary(summary: &DispatchSummary, wt_parent: &Path) -> i32 {
    let _ = std::fs::create_dir_all(wt_parent);
    let json_str = serde_json::to_string_pretty(summary).unwrap();
    let summary_path = wt_parent.join(format!("{}.json", summary.id));
    let _ = std::fs::write(&summary_path, format!("{json_str}\n"));
    println!("{json_str}");
    summary.exit
}

pub fn run_dispatch(args: &DispatchArgs) -> i32 {
    if let Ok(home) = crate::skill::home_dir() {
        if let Ok(Some(path)) = crate::skill::ensure_context_block(&home) {
            eprintln!(
                "blyte: added context rules to {} (they apply from the next Claude session)",
                path.display()
            );
        }
    }

    // 1. Root repository discovery and worktree preparation
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(out) = out else {
        eprintln!("error: git not found");
        return 2;
    };
    if !out.status.success() {
        eprintln!("error: not a git repository");
        return 2;
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    crate::registry::register(&root);

    let (repo_config, from_main) = match crate::config::load_with_fallback(&root) {
        Ok(Some((cfg, from_main))) => (Some(cfg), from_main),
        Ok(None) => (None, false),
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let dispatch_cfg = repo_config.map(|c| c.dispatch).unwrap_or_default();

    let (gate, gate_source) = if args.research {
        ("test -s report.md".to_string(), "research")
    } else if let Some(ref g) = args.gate {
        (g.clone(), "flag")
    } else if let Some(ref g) = dispatch_cfg.gate {
        let toml_path = if from_main {
            crate::config::main_checkout(&root)
                .map(|m| m.join("blyte.toml"))
                .unwrap_or_else(|| root.join("blyte.toml"))
        } else {
            root.join("blyte.toml")
        };
        let toml_content = std::fs::read_to_string(&toml_path).unwrap_or_default();
        let missing = crate::gate_drift::missing_ci_steps(&root, g, &toml_content);
        if let Some(warning) = crate::gate_drift::format_gate_drift_warning(&missing) {
            eprintln!("{warning}");
        }
        let source = if from_main {
            "blyte.toml (main checkout)"
        } else {
            "blyte.toml"
        };
        (g.clone(), source)
    } else {
        eprintln!("no gate: pass --gate, or run blyte init to write blyte.toml");
        return 2;
    };

    let model = args.model.as_ref().or(dispatch_cfg.model.as_ref());
    let (max_seconds, max_turns) = if args.research {
        (
            args.max_seconds.unwrap_or(1800),
            args.max_turns.unwrap_or(300),
        )
    } else {
        (
            args.max_seconds
                .or(dispatch_cfg.max_seconds)
                .unwrap_or(3600),
            args.max_turns.or(dispatch_cfg.max_turns).unwrap_or(300),
        )
    };
    let max_cost = args.max_cost_usd.or(dispatch_cfg.max_cost);
    let fold_budget = args.fold_budget.or(dispatch_cfg.fold_budget);
    let network = args
        .network
        .as_deref()
        .or(dispatch_cfg.network.as_deref())
        .unwrap_or("on");

    let (task_content, recorded_task_file) = if let Some(ref tf) = args.task_file {
        let content = match std::fs::read_to_string(tf) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error reading task file {}: {e}", tf.display());
                return 2;
            }
        };
        (content, Some(tf.display().to_string()))
    } else {
        match (&args.plan, &args.plan_task) {
            (Some(plan_path), Some(plan_task)) => {
                let plan_str = match std::fs::read_to_string(plan_path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("error reading plan {plan_path}: {e}");
                        return 2;
                    }
                };
                let section = match crate::plan_section::extract(&plan_str, plan_task) {
                    Some(sec) => sec,
                    None => {
                        eprintln!("error: no heading \"{plan_task}\" in {plan_path}");
                        if let Some(hint) =
                            crate::plan_section::explain_missing(&plan_str, plan_task)
                        {
                            eprintln!("  {hint}");
                        }
                        return 2;
                    }
                };
                let content = format!("From the plan {plan_path}:\n\n{section}");
                (content, None)
            }
            _ => {
                eprintln!("error: give a task file, or --plan with --plan-task");
                return 2;
            }
        }
    };

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let main_checkout = crate::config::main_checkout(&root).unwrap_or_else(|| root.clone());
    let config_allow_read = match crate::config::expand_allow_read(
        &dispatch_cfg.allow_read,
        home.as_deref(),
        &main_checkout,
    ) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    if !args.no_path_check {
        let mut readable = vec![root.clone(), main_checkout.clone()];
        readable.extend(config_allow_read.iter().cloned());
        readable.extend(args.allow_read.iter().cloned());
        let bad = unreadable_paths(&task_content, &readable);
        if !bad.is_empty() {
            let shown: Vec<String> = bad
                .iter()
                .take(5)
                .map(|p| p.display().to_string())
                .collect();
            eprintln!(
                "blyte: the task names paths this run cannot read: {}. Add --allow-read <dir> for each (or --no-path-check to start anyway).",
                shown.join(", ")
            );
            if args.research {
                return 2;
            }
        }
    }

    let wt_parent = root.join(".wt");
    let worktree_dir = wt_parent.join(&args.id);
    let branch = format!("wip/{}", args.id);
    let task_md_path = wt_parent.join(format!("{}.task.md", args.id));
    let task_file_recorded =
        recorded_task_file.unwrap_or_else(|| task_md_path.display().to_string());

    if !args.research {
        crate::clean::clean_quiet(&root, Some(&args.id));
    }

    if args.research {
        if worktree_dir.exists() {
            eprintln!("error: {} already exists", worktree_dir.display());
            return 2;
        }
        if let Err(e) = std::fs::create_dir_all(&worktree_dir) {
            eprintln!(
                "error creating scratch directory {}: {e}",
                worktree_dir.display()
            );
            return 2;
        }
        let init_out = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&worktree_dir)
            .output();
        match init_out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                eprintln!(
                    "error: git init failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: failed to execute git init: {e}");
                return 2;
            }
        }
        let commit_out = Command::new("git")
            .args([
                "-c",
                "user.name=blyte",
                "-c",
                "user.email=blyte@local",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "research",
            ])
            .current_dir(&worktree_dir)
            .output();
        match commit_out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                eprintln!(
                    "error: git commit failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: failed to execute git commit: {e}");
                return 2;
            }
        }
    } else if !worktree_dir.exists() {
        let add_out = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch,
                worktree_dir.to_str().unwrap(),
                &args.base,
            ])
            .current_dir(&root)
            .output();
        match add_out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                eprintln!(
                    "error: git worktree add failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: failed to execute git worktree add: {e}");
                return 2;
            }
        }
    }

    // 2. Ensure .wt/ is in info/exclude
    let common_dir_raw = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(&root)
        .output();
    let common_dir = if let Ok(out) = common_dir_raw {
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    } else {
        root.join(".git")
    };

    let exclude_path = common_dir.join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if !content.lines().any(|l| l.trim() == ".wt/") {
        let mut new_content = content;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(".wt/\n");
        let _ = std::fs::write(&exclude_path, new_content);
    }

    // 3. Record base_sha
    let base_out = Command::new("git")
        .args(["-C", worktree_dir.to_str().unwrap(), "rev-parse", "HEAD"])
        .output();
    let base_sha = match base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            eprintln!("error: could not resolve HEAD in worktree");
            return 2;
        }
    };

    if !args.research {
        if let Some(ref setup_cmd) = dispatch_cfg.setup {
            let first_line = setup_cmd.lines().next().unwrap_or("");
            eprintln!("setup: {first_line}");
            let main_checkout = crate::config::main_checkout(&root).unwrap_or_else(|| root.clone());
            let setup_log_path = wt_parent.join(format!("{}.setup.log", args.id));
            if let Err(msg) = crate::setup::run_setup(
                setup_cmd,
                &worktree_dir,
                &main_checkout,
                &dispatch_cfg.env,
                &setup_log_path,
                crate::setup::SETUP_TIMEOUT,
            ) {
                let outcome = "blocked";
                let exit = exit_code(outcome);
                let first_line_err = msg.lines().next().unwrap_or("").to_string();
                let summary = DispatchSummary {
                    id: args.id.clone(),
                    worktree: worktree_dir.display().to_string(),
                    branch: branch.clone(),
                    outcome,
                    exit,
                    error: Some(first_line_err),
                    commits: Vec::new(),
                    changed_files: Vec::new(),
                    final_message: Some(msg),
                    turns: 0,
                    gate_attempts: 0,
                    gate_failures: Vec::new(),
                    cost_usd: None,
                    model: None,
                    seconds: 0,
                    sandbox: args.sandbox.clone(),
                    log: setup_log_path.display().to_string(),
                    report: "".to_string(),
                    gate_source,
                    research_report: None,
                };
                return finish_summary(&summary, &wt_parent);
            }
        }
    }

    // 4. Write task file and spawn child process
    let _ = std::fs::create_dir_all(&wt_parent);
    if let Err(e) = std::fs::write(&task_md_path, &task_content) {
        eprintln!("error writing task file {}: {e}", task_md_path.display());
        return 2;
    }

    let log_path = wt_parent.join(format!("{}.log", args.id));
    let report_path = wt_parent.join(format!("{}.report.json", args.id));
    if report_path.exists() {
        let _ = std::fs::remove_file(&report_path);
    }
    let research_report_path = wt_parent.join(format!("{}.report.md", args.id));
    if research_report_path.exists() {
        let _ = std::fs::remove_file(&research_report_path);
    }

    let log_file = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error creating log file {}: {e}", log_path.display());
            return 2;
        }
    };
    let log_file_err = match log_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error cloning log file handle: {e}");
            return 2;
        }
    };

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error getting current executable: {e}");
            return 2;
        }
    };

    let mut cmd = Command::new(current_exe);
    cmd.current_dir(&worktree_dir);
    cmd.stdout(log_file);
    cmd.stderr(log_file_err);

    for (k, v) in &dispatch_cfg.env {
        cmd.env(k, v);
    }
    cmd.env("BLYTE_DISPATCH_ID", &args.id);

    cmd.arg("--gate").arg(&gate);
    cmd.arg("--rollback=false");
    cmd.arg("--max-seconds").arg(max_seconds.to_string());
    cmd.arg("--max-turns").arg(max_turns.to_string());
    cmd.arg("--report").arg(&report_path);

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    if let Some(c) = max_cost {
        cmd.arg("--max-cost").arg(c.to_string());
    }
    if let Some(fb) = fold_budget {
        cmd.arg("--fold-budget").arg(fb.to_string());
    }
    cmd.arg("--sandbox").arg(&args.sandbox);
    cmd.arg("--network").arg(network);
    for r in &config_allow_read {
        cmd.arg("--allow-read").arg(r);
    }
    for r in &args.allow_read {
        cmd.arg("--allow-read").arg(r);
    }
    for p in &args.pass_env {
        cmd.arg("--pass-env").arg(p);
    }
    // The read and grep tools may read what the sandbox may read.
    for r in config_allow_read.iter().chain(&args.allow_read) {
        cmd.arg("--read-root").arg(r);
    }
    if args.research {
        cmd.arg("--read-root").arg(&root);
        cmd.arg("--no-gate-slot");
        cmd.arg("--research");
    }

    cmd.arg("--");
    let task_prompt = if args.research {
        wrap_research(&task_content, &root)
    } else {
        let mut prompt = wrap_task(&task_content);
        if let Some(qs) = quick_steps(&gate) {
            prompt.push_str("\n\n");
            prompt.push_str(&qs);
        }
        prompt
    };
    cmd.arg(task_prompt);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error spawning child process: {e}");
            return 2;
        }
    };

    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let env_names: Vec<&str> = dispatch_cfg.env.keys().map(String::as_str).collect();
    let mut tags = args.tags.clone();
    if args.research {
        tags.push("research".to_string());
    }
    let mut start_json = serde_json::json!({
        "id": args.id,
        "pid": child.id(),
        "started_at": started_at,
        "task_file": task_file_recorded,
        "gate": gate,
        "tags": tags,
        "env": env_names,
        "max_turns": max_turns,
        "max_seconds": max_seconds,
        "plan": args.plan,
        "plan_task": args.plan_task,
        "base": args.base,
    });
    if let Some(session) = crate::usage_log::session_from(
        std::env::var("BLYTE_SESSION").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    ) {
        start_json["session"] = serde_json::json!(session);
    }
    if args.research {
        start_json["research"] = serde_json::json!(true);
    }
    let start_path = wt_parent.join(format!("{}.start.json", args.id));
    if let Ok(json_str) = serde_json::to_string_pretty(&start_json) {
        let _ = std::fs::write(&start_path, json_str + "\n");
    }

    let _ = child.wait();

    let scratch_report = worktree_dir.join("report.md");
    if args.research {
        let is_non_empty = std::fs::metadata(&scratch_report)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if is_non_empty {
            let _ = std::fs::copy(&scratch_report, &research_report_path);
            if let Some(ref report_to) = args.report_to {
                if let Some(parent) = report_to.parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                let _ = std::fs::copy(&scratch_report, report_to);
            }
        }
    }

    // 5. Read report, commits and changed files
    let log_out = Command::new("git")
        .args([
            "-C",
            worktree_dir.to_str().unwrap(),
            "log",
            "--reverse",
            "--format=%h%x09%s",
            &format!("{base_sha}..HEAD"),
        ])
        .output();

    let mut commits = Vec::new();
    if let Ok(o) = log_out {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((sha, subject)) = line.split_once('\t') {
                commits.push(CommitRecord {
                    sha: sha.to_string(),
                    subject: subject.to_string(),
                });
            }
        }
    }

    let diff_out = Command::new("git")
        .args([
            "-C",
            worktree_dir.to_str().unwrap(),
            "diff",
            "--name-only",
            &format!("{base_sha}..HEAD"),
        ])
        .output();

    let mut changed_files = Vec::new();
    if let Ok(o) = diff_out {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            let line = line.trim();
            if !line.is_empty() {
                changed_files.push(line.to_string());
            }
        }
    }

    let report_read = std::fs::read_to_string(&report_path);
    let record: Result<serde_json::Value, String> = match report_read {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("unreadable report JSON: {e}")),
        Err(e) => Err(format!("missing report: {e}")),
    };

    let (
        report_outcome,
        error,
        final_message,
        turns,
        gate_attempts,
        gate_failures,
        cost_usd,
        model,
        seconds,
        sandbox,
    ) = match record {
        Ok(rec) => {
            let gate_failures = rec
                .get("gate_failures")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| {
                            if s.chars().count() > 1000 {
                                s.chars().take(1000).collect()
                            } else {
                                s.to_string()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            (
                rec.get("outcome")
                    .and_then(|v| v.as_str())
                    .unwrap_or("error")
                    .to_string(),
                rec.get("error").and_then(|v| v.as_str()).map(String::from),
                rec.get("final_message")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                rec.get("turns").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                rec.get("gate_attempts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                gate_failures,
                rec.get("cost_usd").and_then(|v| v.as_f64()),
                rec.get("model_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                rec.get("duration_seconds")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .round() as u64,
                rec.get("sandbox")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&args.sandbox)
                    .to_string(),
            )
        }
        Err(err_msg) => (
            "error".to_string(),
            Some(err_msg),
            None,
            0,
            0,
            Vec::new(),
            None,
            None,
            0,
            args.sandbox.clone(),
        ),
    };

    let outcome = if args.research {
        match report_outcome.as_str() {
            "verified" => "verified",
            "gate_failed" => "gate_failed",
            "budget_exhausted" => "budget_exhausted",
            "blocked" => "blocked",
            _ => "error",
        }
    } else {
        dispatch_outcome(&report_outcome, commits.len())
    };
    let exit = exit_code(outcome);

    let research_report = if args.research && research_report_path.exists() {
        Some(research_report_path.display().to_string())
    } else {
        None
    };

    // 6. Print JSON summary
    let summary = DispatchSummary {
        id: args.id.clone(),
        worktree: worktree_dir.display().to_string(),
        branch,
        outcome,
        exit,
        error,
        commits,
        changed_files,
        final_message,
        turns,
        gate_attempts,
        gate_failures,
        cost_usd,
        model,
        seconds,
        sandbox,
        log: log_path.display().to_string(),
        report: report_path.display().to_string(),
        gate_source,
        research_report,
    };

    finish_summary(&summary, &wt_parent)
}

pub fn run_review(args: &ReviewArgs) -> i32 {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(out) = out else {
        eprintln!("error: git not found");
        return 2;
    };
    if !out.status.success() {
        eprintln!("error: not a git repository");
        return 2;
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());

    if args.notes.is_some() && args.verdict.is_none() {
        eprintln!("error: --notes requires --verdict");
        return 2;
    }

    if let Some(ref v) = args.verdict {
        let wt_dir = root.join(".wt");
        if let Err(e) = std::fs::create_dir_all(&wt_dir) {
            eprintln!("error creating directory {}: {e}", wt_dir.display());
            return 2;
        }
        let verdict_path = wt_dir.join(format!("{}.verdict.json", args.id));
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let verdict_obj = serde_json::json!({
            "verdict": v.as_str(),
            "notes": args.notes.as_deref().unwrap_or(""),
            "ts": ts,
        });
        let json_str = match serde_json::to_string_pretty(&verdict_obj) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error serializing verdict: {e}");
                return 2;
            }
        };
        if let Err(e) = std::fs::write(&verdict_path, json_str + "\n") {
            eprintln!("error writing {}: {e}", verdict_path.display());
            return 2;
        }
        println!("recorded verdict {} for {}", v.as_str(), args.id);
        crate::clean::clean_on_review(&root, &args.id);
        return 0;
    }

    let wt = root.join(".wt").join(&args.id);
    if !wt.exists() {
        eprintln!("error: worktree {} does not exist", wt.display());
        return 2;
    }

    let root_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output();
    let root_head_sha = match root_head {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            eprintln!("error: could not resolve root HEAD");
            return 2;
        }
    };

    let mb_out = Command::new("git")
        .args(["merge-base", "HEAD", &root_head_sha])
        .current_dir(&wt)
        .output();
    let base = match mb_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            let has_root_head = Command::new("git")
                .args(["cat-file", "-e", &format!("{root_head_sha}^{{commit}}")])
                .current_dir(&wt)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if has_root_head {
                root_head_sha
            } else {
                Command::new("git")
                    .args(["rev-list", "--max-parents=0", "HEAD"])
                    .current_dir(&wt)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string()
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or(root_head_sha)
            }
        }
    };

    let commits_out = Command::new("git")
        .args(["log", "--oneline", &format!("{base}..HEAD")])
        .current_dir(&wt)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let files_out = Command::new("git")
        .args(["diff", "--stat", &format!("{base}..HEAD")])
        .current_dir(&wt)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let tree_out = Command::new("git")
        .args(["status", "--short"])
        .current_dir(&wt)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let diff_out = if args.diff {
        Command::new("git")
            .args(["diff", &format!("{base}..HEAD")])
            .current_dir(&wt)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let task_text: Option<String> = (|| {
        let start_file = root.join(".wt").join(format!("{}.start.json", args.id));
        let content = std::fs::read_to_string(&start_file).ok()?;
        let val: serde_json::Value = serde_json::from_str(&content).ok()?;
        let task_file = val.get("task_file")?.as_str()?;
        let p = Path::new(task_file);
        if let Ok(s) = std::fs::read_to_string(p) {
            return Some(s);
        }
        std::fs::read_to_string(root.join(p)).ok()
    })();

    let changed_files: Vec<String> = Command::new("git")
        .args(["diff", "--name-only", &format!("{base}..HEAD")])
        .current_dir(&wt)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let subjects: Vec<String> = Command::new("git")
        .args(["log", "--format=%s", &format!("{base}..HEAD")])
        .current_dir(&wt)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let log_file = root.join(".wt").join(format!("{}.log", args.id));
    let (hcmds, reads, (refused_count, refused_paths)) =
        if let Ok(log_content) = std::fs::read_to_string(&log_file) {
            let hcmds = host_commands(&log_content);
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            let allowed = [root.clone(), wt.clone()];
            let reads = outside_reads(&log_content, &allowed, &home);
            let refused = refused_reads(&log_content);
            (hcmds, reads, refused)
        } else {
            (Vec::new(), Vec::new(), (0, Vec::new()))
        };

    let report_file = root.join(".wt").join(format!("{}.report.json", args.id));
    let report_val = std::fs::read_to_string(&report_file)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok());
    let outcome = report_val
        .as_ref()
        .and_then(|v| v.get("outcome"))
        .and_then(|x| x.as_str());
    let quote_problems: Option<Vec<&str>> = report_val
        .as_ref()
        .and_then(|v| v.get("quote_problems"))
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str()).collect());
    let read_loop = report_val
        .as_ref()
        .and_then(|v| v.get("read_loop"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);

    let mut checks = crate::review_checks::build(
        task_text.as_deref(),
        &changed_files,
        &subjects,
        hcmds.len(),
        reads.len(),
        outcome,
    );
    if let Some(ref problems) = quote_problems {
        if !problems.is_empty() {
            checks.flags.push(format!(
                "quote check failed ({} problem(s)): see == report",
                problems.len()
            ));
        }
    }
    if read_loop {
        checks.flags.push(
            "read loop: the run kept re-reading the same files without changing anything and was stopped"
                .to_string(),
        );
    }
    if refused_count > 0 {
        let first_3: Vec<&str> = refused_paths.iter().take(3).map(|s| s.as_str()).collect();
        checks.flags.push(format!(
            "refused reads: {refused_count} (outside the workspace and --allow-read): {}",
            first_3.join(", ")
        ));
    }

    println!("== checks");
    if checks.flags.is_empty() {
        println!("nothing flagged");
    } else {
        for flag in &checks.flags {
            println!("! {flag}");
        }
    }

    println!("== commits");
    if !commits_out.is_empty() {
        println!("{commits_out}");
    }
    println!("== files");
    if !files_out.is_empty() {
        println!("{files_out}");
    }
    println!("== tree");
    if tree_out.is_empty() {
        println!("(clean)");
    } else {
        println!("{tree_out}");
    }
    println!("== report");
    if let Some(ref v) = report_val {
        if let Some(outcome) = v.get("outcome").and_then(|x| x.as_str()) {
            println!("outcome: {outcome}");
        }
        if let Some(turns) = v.get("turns").and_then(|x| x.as_u64()) {
            println!("turns: {turns}");
        }
        if let Some(cost) = v.get("cost_usd").and_then(|x| x.as_f64()) {
            println!("cost_usd: {cost}");
        }
        if let Some(msg) = v.get("final_message").and_then(|x| x.as_str()) {
            println!("final_message: {msg}");
        }
        if let Some(ref problems) = quote_problems {
            if problems.is_empty() {
                println!("quote check: passed");
            } else {
                println!(
                    "quote check: FAILED, {} problems (the report's quotes do not match the code)",
                    problems.len()
                );
                for p in problems.iter().take(5) {
                    let cut: String = p.chars().take(200).collect();
                    println!("  {cut}");
                }
            }
        }
    }

    let research_report_file = root.join(".wt").join(format!("{}.report.md", args.id));
    if research_report_file.exists() {
        println!("== research report");
        println!("{}", research_report_file.display());
        if let Ok(content) = std::fs::read_to_string(&research_report_file) {
            for line in content.lines().take(40) {
                println!("{line}");
            }
        }
    }

    if !hcmds.is_empty() {
        println!("== host commands (may have changed things outside the worktree)");
        for cmd in hcmds {
            println!("{cmd}");
        }
    }
    if !reads.is_empty() {
        println!("== reads outside the repository");
        for cmd in reads {
            println!("{cmd}");
        }
    }

    if args.diff {
        println!("== diff");
        if !diff_out.is_empty() {
            println!("{diff_out}");
        }
    }

    if args.model_review {
        crate::model_review::run(&root, &args.id, &base, &wt);
    }

    0
}

const OUTSIDE_PATHS: &[&str] = &[
    "~/",
    "$HOME",
    "/Users/",
    "/home/",
    "/private/",
    "/var/",
    "/etc/",
    "/opt/",
    "/usr/",
];

const CHANGE_WORDS: &[&str] = &[
    "rm", "rmdir", "mv", "cp", "chmod", "chown", "truncate", "dd", "tee", "ln",
];

fn contains_word(cmd: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = cmd[start..].find(word) {
        let abs_pos = start + pos;
        let before_ok = if abs_pos == 0 {
            true
        } else {
            let prev = cmd[..abs_pos].chars().next_back().unwrap();
            !prev.is_alphanumeric() && prev != '_' && prev != '-'
        };
        let after_pos = abs_pos + word.len();
        let after_ok = if after_pos >= cmd.len() {
            true
        } else {
            let next = cmd[after_pos..].chars().next().unwrap();
            !next.is_alphanumeric() && next != '_' && next != '-'
        };
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + word.len();
    }
    false
}

fn has_non_dev_null_redirect(cmd: &str) -> bool {
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '>' {
            while i < chars.len() && chars[i] == '>' {
                i += 1;
            }
            if i < chars.len() && chars[i] == '|' {
                i += 1;
            }
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
            if chars[i] == '&'
                && i + 1 < chars.len()
                && (chars[i + 1].is_ascii_digit() || chars[i + 1] == '-')
            {
                i += 1;
                continue;
            }
            let target_start = i;
            while i < chars.len()
                && !chars[i].is_whitespace()
                && chars[i] != ';'
                && chars[i] != '|'
                && chars[i] != '&'
                && chars[i] != ')'
            {
                i += 1;
            }
            let target: String = chars[target_start..i].iter().collect();
            let target = target.trim_matches(|c| c == '\'' || c == '"');
            if !target.is_empty() && target != "/dev/null" {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

fn changes_something(cmd: &str) -> bool {
    for &word in CHANGE_WORDS {
        if contains_word(cmd, word) {
            return true;
        }
    }
    has_non_dev_null_redirect(cmd)
}

fn is_host_command(cmd: &str) -> bool {
    if cmd.contains("git push")
        || cmd.contains("sudo ")
        || cmd.contains("cache clean")
        || cmd.contains("cache prune")
        || cmd.contains("brew ")
    {
        return true;
    }

    let names_outside = OUTSIDE_PATHS.iter().any(|path| cmd.contains(path));
    if names_outside && changes_something(cmd) {
        return true;
    }

    false
}

pub fn host_commands(log: &str) -> Vec<String> {
    let cleaned = strip_ansi(log);
    let mut result = Vec::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("● bash ") {
            if let Some((cmd, _)) = rest.rsplit_once(" -> ") {
                let cmd = cmd.trim();
                if is_host_command(cmd) && !result.iter().any(|c| c == cmd) {
                    result.push(cmd.to_string());
                    if result.len() == 20 {
                        break;
                    }
                }
            }
        }
    }
    result
}

const TERMINAL_READS: &[&str] = &[
    "herdr pane read",
    "tmux capture-pane",
    "screen -X hardcopy",
    "zellij action dump-screen",
    "wezterm cli get-text",
];

const TOOL_CACHES: &[&str] = &[
    ".cargo",
    ".rustup",
    ".cache",
    ".npm",
    ".local",
    "Library/Caches",
    ".gradle",
    ".m2",
    "go",
];

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => out.push(comp),
        }
    }
    out
}

fn is_outside_read(cmd: &str, allowed: &[PathBuf], home: &Path, tool_caches: &[PathBuf]) -> bool {
    if TERMINAL_READS.iter().any(|pattern| cmd.contains(pattern)) {
        return true;
    }

    for raw_word in cmd.split_whitespace() {
        let word = raw_word.trim_end_matches(&[':', ',', ')', '\'', '"'][..]);
        let raw_path = if let Some(suffix) = word.strip_prefix("~/") {
            Some(home.join(suffix))
        } else if let Some(suffix) = word.strip_prefix("$HOME/") {
            Some(home.join(suffix))
        } else if word.starts_with('/') {
            Some(PathBuf::from(word))
        } else {
            None
        };

        if let Some(path) = raw_path {
            let norm_path = normalize_path(&path);
            if !norm_path.starts_with(home) {
                continue;
            }
            let under_allowed = allowed.iter().any(|a| norm_path.starts_with(a));
            if under_allowed {
                continue;
            }
            let under_cache = tool_caches.iter().any(|c| norm_path.starts_with(c));
            if under_cache {
                continue;
            }
            return true;
        }
    }

    false
}

pub fn outside_reads(log: &str, allowed: &[PathBuf], home: &Path) -> Vec<String> {
    let host_cmds = host_commands(log);
    let norm_home = normalize_path(home);
    let norm_allowed: Vec<PathBuf> = allowed.iter().map(|p| normalize_path(p)).collect();
    let cache_dirs: Vec<PathBuf> = TOOL_CACHES
        .iter()
        .map(|c| normalize_path(&norm_home.join(c)))
        .collect();

    let cleaned = strip_ansi(log);
    let mut result = Vec::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("● bash ") {
            if let Some((cmd, _)) = rest.rsplit_once(" -> ") {
                let cmd = cmd.trim();
                if host_cmds.iter().any(|c| c == cmd) {
                    continue;
                }
                if is_outside_read(cmd, &norm_allowed, &norm_home, &cache_dirs)
                    && !result.iter().any(|c| c == cmd)
                {
                    result.push(cmd.to_string());
                    if result.len() == 20 {
                        break;
                    }
                }
            }
        }
    }
    result
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&next_c) = chars.peek() {
                chars.next();
                if ('\x40'..='\x7e').contains(&next_c) {
                    break;
                }
                if next_c == '\n' || next_c == '\x1b' {
                    out.push(next_c);
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogProgress {
    pub turn: u32,
    pub last_tool: Option<String>,
    pub gate_attempts: u32,
    pub gate_passed: bool,
}

pub fn parse_log(log: &str) -> LogProgress {
    let cleaned = strip_ansi(log);
    let mut turn: u32 = 0;
    let mut last_tool: Option<String> = None;
    let mut gate_attempts: u32 = 0;
    let mut gate_passed: bool = false;

    for line in cleaned.lines() {
        if let Some(rest) = line.strip_prefix("[Turn ") {
            if let Some((n_str, _)) = rest.split_once(']') {
                if let Ok(n) = n_str.trim().parse::<u32>() {
                    turn = turn.max(n);
                }
            }
        }

        if let Some(tool) = line.strip_prefix("● ") {
            let truncated = if tool.chars().count() > 200 {
                tool.chars().take(200).collect()
            } else {
                tool.to_string()
            };
            last_tool = Some(truncated);
        }

        if line.starts_with("✓ Gate passed") {
            gate_attempts += 1;
            gate_passed = true;
        } else if line.starts_with("✗ Gate failed") {
            gate_attempts += 1;
            gate_passed = false;
        }
    }

    LogProgress {
        turn,
        last_tool,
        gate_attempts,
        gate_passed,
    }
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub id: String,
    pub state: String,
    pub elapsed_s: u64,
    pub turn: u32,
    pub last_tool: Option<String>,
    pub gate_attempts: u32,
    pub gate_passed: bool,
    pub outcome: Option<String>,
    pub cost_usd: Option<f64>,
    pub final_message: Option<String>,
}

pub fn run_status(id: &str, json: bool) -> i32 {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(out) = out else {
        eprintln!("error: git not found");
        return 2;
    };
    if !out.status.success() {
        eprintln!("error: not a git repository");
        return 2;
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());

    let start_path = root.join(".wt").join(format!("{id}.start.json"));
    if !start_path.exists() {
        eprintln!("no dispatch {id} here");
        return 2;
    }

    let start_record: serde_json::Value = match std::fs::read_to_string(&start_path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing {}: {e}", start_path.display());
                return 2;
            }
        },
        Err(e) => {
            eprintln!("error reading {}: {e}", start_path.display());
            return 2;
        }
    };

    let pid = start_record
        .get("pid")
        .and_then(|v| v.as_i64())
        .map(|v| v as libc::pid_t);
    let started_at = start_record
        .get("started_at")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let is_alive = if let Some(pid) = pid {
        if pid > 0 {
            let ret = unsafe { libc::kill(pid, 0) };
            if ret == 0 {
                true
            } else {
                std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
            }
        } else {
            false
        }
    } else {
        false
    };

    let report_path = root.join(".wt").join(format!("{id}.report.json"));
    let report_exists = report_path.exists();

    let state = if report_exists {
        "finished"
    } else if is_alive {
        "running"
    } else {
        "dead"
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (outcome, cost_usd, final_message, report_duration) = if state == "finished" {
        if let Ok(content) = std::fs::read_to_string(&report_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                let outcome = v.get("outcome").and_then(|x| x.as_str()).map(String::from);
                let cost_usd = v.get("cost_usd").and_then(|x| x.as_f64());
                let final_message = v.get("final_message").and_then(|x| x.as_str()).map(|s| {
                    if s.chars().count() > 500 {
                        s.chars().take(500).collect()
                    } else {
                        s.to_string()
                    }
                });
                let duration = v.get("duration_seconds").and_then(|x| x.as_f64());
                (outcome, cost_usd, final_message, duration)
            } else {
                (None, None, None, None)
            }
        } else {
            (None, None, None, None)
        }
    } else {
        (None, None, None, None)
    };

    let elapsed_s = if state == "finished" {
        report_duration
            .map(|d| d.round() as u64)
            .unwrap_or_else(|| now.saturating_sub(started_at))
    } else {
        now.saturating_sub(started_at)
    };

    let log_path = root.join(".wt").join(format!("{id}.log"));
    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();
    let progress = parse_log(&log_content);

    if json {
        let status_obj = StatusOutput {
            id: id.to_string(),
            state: state.to_string(),
            elapsed_s,
            turn: progress.turn,
            last_tool: progress.last_tool,
            gate_attempts: progress.gate_attempts,
            gate_passed: progress.gate_passed,
            outcome,
            cost_usd,
            final_message,
        };
        let json_str = serde_json::to_string_pretty(&status_obj).unwrap();
        println!("{json_str}");
    } else {
        println!("id: {id}");
        println!("state: {state}");
        println!("elapsed_s: {elapsed_s}");
        println!("turn: {}", progress.turn);
        println!(
            "last_tool: {}",
            progress.last_tool.as_deref().unwrap_or("none")
        );
        println!("gate_attempts: {}", progress.gate_attempts);
        println!("gate_passed: {}", progress.gate_passed);
        if state == "finished" {
            if let Some(ref o) = outcome {
                println!("outcome: {o}");
            }
            if let Some(c) = cost_usd {
                println!("cost_usd: {c}");
            }
            if let Some(ref msg) = final_message {
                println!("final_message: {msg}");
            }
        }
    }

    match state {
        "running" | "finished" => 0,
        _ => 1,
    }
}

pub fn unreadable_paths(text: &str, readable: &[PathBuf]) -> Vec<PathBuf> {
    let readable_canon: Vec<PathBuf> = readable
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .collect();
    let mut out = Vec::new();
    for raw in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
            )
    }) {
        let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if !trimmed.starts_with('/')
            || trimmed.starts_with("//")
            || trimmed.len() <= 1
            || trimmed == "/dev/null"
        {
            continue;
        }
        let p = Path::new(trimmed);
        let Ok(canon) = std::fs::canonicalize(p) else {
            continue;
        };
        if readable_canon.iter().any(|r| canon.starts_with(r))
            || is_system_path(p)
            || is_system_path(&canon)
        {
            continue;
        }
        let pb = PathBuf::from(trimmed);
        if !out.contains(&pb) {
            out.push(pb);
        }
    }
    out
}

/// A path no research task means as code to read: the filesystem root, a top-level directory,
/// or anything under the system directories (`/usr/bin/env`, `/tmp`, `/dev/null`).
fn is_system_path(p: &Path) -> bool {
    const SYSTEM: &[&str] = &[
        "/usr",
        "/bin",
        "/sbin",
        "/etc",
        "/dev",
        "/opt",
        "/System",
        "/Library",
        "/Applications",
        "/nix",
        "/proc",
        "/sys",
    ];
    p.components().count() <= 2 || SYSTEM.iter().any(|s| p.starts_with(s))
}

pub fn refused_reads(log_content: &str) -> (usize, Vec<String>) {
    let mut count = 0;
    let mut paths = Vec::new();
    for line in log_content.lines() {
        if !line.contains("path escapes the workspace") {
            continue;
        }
        count += 1;
        if let Some((_, after)) = line.split_once("path escapes the workspace:") {
            let after = after.trim_start();
            let after = after.split("\x1b[").next().unwrap_or(after);
            let path = after
                .split(". This run can read")
                .next()
                .unwrap_or(after)
                .trim();
            if !path.is_empty() && !paths.iter().any(|p| p == path) {
                paths.push(path.to_string());
            }
        }
    }
    (count, paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_reads_excludes_commands_in_host_commands() {
        let home = Path::new("/Users/u");
        let allowed = vec![PathBuf::from("/Users/u/repos/app")];
        // rm -rf ~/repos/other is a host command (names outside and changes something)
        // cat ~/repos/other is not a host command (only reads)
        let log = "\
● bash rm -rf ~/repos/other/file.py -> exit 0
● bash cat ~/repos/other/file.py -> exit 0
";
        assert_eq!(
            host_commands(log),
            vec!["rm -rf ~/repos/other/file.py".to_string()]
        );
        assert_eq!(
            outside_reads(log, &allowed, home),
            vec!["cat ~/repos/other/file.py".to_string()]
        );
    }

    #[test]
    fn outside_reads_all_terminal_patterns() {
        let home = Path::new("/Users/u");
        let allowed = vec![PathBuf::from("/Users/u/repos/app")];
        let log = "\
● bash herdr pane read p1 -> exit 0
● bash tmux capture-pane -t 1 -> exit 0
● bash screen -X hardcopy /tmp/screen.log -> exit 0
● bash zellij action dump-screen /tmp/dump -> exit 0
● bash wezterm cli get-text --pane-id 1 -> exit 0
";
        let reads = outside_reads(log, &allowed, home);
        assert_eq!(
            reads,
            vec![
                "herdr pane read p1",
                "tmux capture-pane -t 1",
                "screen -X hardcopy /tmp/screen.log",
                "zellij action dump-screen /tmp/dump",
                "wezterm cli get-text --pane-id 1",
            ]
        );
    }

    #[test]
    fn outside_reads_all_tool_caches_ignored() {
        let home = Path::new("/Users/u");
        let allowed = vec![PathBuf::from("/Users/u/repos/app")];
        let log = "\
● bash ls ~/.cargo/bin -> exit 0
● bash ls ~/.rustup/toolchains -> exit 0
● bash ls ~/.cache/pip -> exit 0
● bash ls ~/.npm/_cacache -> exit 0
● bash ls ~/.local/share -> exit 0
● bash ls ~/Library/Caches/com.apple -> exit 0
● bash ls ~/.gradle/caches -> exit 0
● bash ls ~/.m2/repository -> exit 0
● bash ls ~/go/pkg -> exit 0
";
        assert!(outside_reads(log, &allowed, home).is_empty());
    }

    #[test]
    fn outside_reads_trailing_punctuations_stripped() {
        let home = Path::new("/Users/u");
        let allowed = vec![PathBuf::from("/Users/u/repos/app")];
        let log = "\
● bash ls ~/secret: -> exit 0
● bash ls ~/secret, -> exit 0
● bash (ls ~/secret) -> exit 0
● bash bash -c 'ls ~/secret' -> exit 0
● bash bash -c \"ls ~/secret\" -> exit 0
";
        let reads = outside_reads(log, &allowed, home);
        // Deduplicated, so only one instance of the command or distinct commands:
        assert_eq!(reads.len(), 5);
    }

    #[test]
    fn outside_reads_caps_at_20_and_deduplicates() {
        let home = Path::new("/Users/u");
        let allowed = vec![PathBuf::from("/Users/u/repos/app")];
        let mut log = String::new();
        for i in 0..30 {
            log.push_str(&format!("● bash cat ~/secret/{i}.txt -> exit 0\n"));
        }
        // Duplicate the first one
        log.push_str("● bash cat ~/secret/0.txt -> exit 0\n");
        let reads = outside_reads(&log, &allowed, home);
        assert_eq!(reads.len(), 20);
        assert_eq!(reads[0], "cat ~/secret/0.txt");
        assert_eq!(reads[19], "cat ~/secret/19.txt");
    }
}
