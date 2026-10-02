//! `blyte dispatch` and `blyte review` commands.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{DispatchArgs, ReviewArgs};

pub const DISPATCH_RULES: &str = r#"Rules:
- Stay within the task. If it cannot pass without a small change elsewhere, make the smallest one and say so.
- Never delete, clean or modify anything outside the worktree, and never change the machine to make a check pass. If a check fails because of the environment (disk, network, a missing service), stop and say so.
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
- Read the code with the read and grep tools, using absolute paths under the repository; use the shell only when they cannot do what you need.
- Answer the question with evidence: file paths with line numbers, and quote only what is needed.
- Write your findings to `report.md` in your working directory, then stop calling tools and reply with one line summarising them. The harness checks that `report.md` exists.
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
    pub seconds: u64,
    pub sandbox: String,
    pub log: String,
    pub report: String,
    pub gate_source: &'static str,
    pub research_report: Option<String>,
}

pub fn run_dispatch(args: &DispatchArgs) -> i32 {
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
        let missing = missing_ci_steps(&root, g, &toml_content);
        if let Some(warning) = format_gate_drift_warning(&missing) {
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

    let wt_parent = root.join(".wt");
    let worktree_dir = wt_parent.join(&args.id);
    let branch = format!("wip/{}", args.id);

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

    // 4. Read task file and spawn child process
    let task_content = match std::fs::read_to_string(&args.task_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading task file {}: {e}", args.task_file.display());
            return 2;
        }
    };

    let _ = std::fs::create_dir_all(&wt_parent);
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
    for r in &args.allow_read {
        cmd.arg("--allow-read").arg(r);
    }
    for p in &args.pass_env {
        cmd.arg("--pass-env").arg(p);
    }
    if args.research {
        cmd.arg("--read-root").arg(&root);
        cmd.arg("--no-gate-slot");
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
        "task_file": args.task_file.display().to_string(),
        "gate": gate,
        "tags": tags,
        "env": env_names,
        "max_turns": max_turns,
        "max_seconds": max_seconds,
        "plan": args.plan,
        "plan_task": args.plan_task,
        "base": args.base,
    });
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
        seconds,
        sandbox,
        log: log_path.display().to_string(),
        report: report_path.display().to_string(),
        gate_source,
        research_report,
    };

    let json_str = serde_json::to_string_pretty(&summary).unwrap();
    let summary_path = wt_parent.join(format!("{}.json", args.id));
    let _ = std::fs::write(&summary_path, format!("{json_str}\n"));
    println!("{json_str}");
    exit
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

    let report_file = root.join(".wt").join(format!("{}.report.json", args.id));

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
    if let Ok(content) = std::fs::read_to_string(&report_file) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
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

    let log_file = root.join(".wt").join(format!("{}.log", args.id));
    if let Ok(log_content) = std::fs::read_to_string(&log_file) {
        let hcmds = host_commands(&log_content);
        if !hcmds.is_empty() {
            println!("== host commands (may have changed things outside the worktree)");
            for cmd in hcmds {
                println!("{cmd}");
            }
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let allowed = [root.clone(), wt.clone()];
        let reads = outside_reads(&log_content, &allowed, &home);
        if !reads.is_empty() {
            println!("== reads outside the repository");
            for cmd in reads {
                println!("{cmd}");
            }
        }
    }

    if args.diff {
        println!("== diff");
        if !diff_out.is_empty() {
            println!("{diff_out}");
        }
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CheckKey {
    Make(String),
    Cargo(String),
    Go(String),
    Npm(String),
    Pnpm(String),
    Yarn(String),
    Ruff(String),
    Tool(String),
}

impl CheckKey {
    pub fn key_text(&self) -> String {
        match self {
            CheckKey::Make(target) => format!("make {target}"),
            CheckKey::Cargo(subcmd) => format!("cargo {subcmd}"),
            CheckKey::Go(subcmd) => format!("go {subcmd}"),
            CheckKey::Npm(script) => format!("npm run {script}"),
            CheckKey::Pnpm(script) => format!("pnpm run {script}"),
            CheckKey::Yarn(script) => format!("yarn {script}"),
            CheckKey::Ruff(subcmd) => format!("ruff {subcmd}"),
            CheckKey::Tool(tool) => tool.clone(),
        }
    }

    pub fn matches(&self, other: &CheckKey) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            (CheckKey::Npm(s1), CheckKey::Pnpm(s2))
            | (CheckKey::Npm(s1), CheckKey::Yarn(s2))
            | (CheckKey::Pnpm(s1), CheckKey::Npm(s2))
            | (CheckKey::Pnpm(s1), CheckKey::Yarn(s2))
            | (CheckKey::Yarn(s1), CheckKey::Npm(s2))
            | (CheckKey::Yarn(s1), CheckKey::Pnpm(s2)) => s1 == s2,
            _ => false,
        }
    }
}

pub fn split_pipeline_and_chain(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        match in_quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    in_quote = None;
                }
                i += 1;
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                    current.push(ch);
                    i += 1;
                } else if (ch == '&' && i + 1 < chars.len() && chars[i + 1] == '&')
                    || (ch == '|' && i + 1 < chars.len() && chars[i + 1] == '|')
                {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 2;
                } else if ch == ';' || ch == '|' {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 1;
                } else {
                    current.push(ch);
                    i += 1;
                }
            }
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

fn tokenize_cmd(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;

    for ch in s.chars() {
        match in_quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    in_quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                    current.push(ch);
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        tokens.push(current);
                        current = String::new();
                    }
                } else {
                    current.push(ch);
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn unquote(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn is_env_assignment(s: &str) -> bool {
    let Some(eq_idx) = s.find('=') else {
        return false;
    };
    if eq_idx == 0 {
        return false;
    }
    let key = &s[..eq_idx];
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_uv_flag_with_arg(flag: &str) -> bool {
    if flag.contains('=') {
        return false;
    }
    matches!(
        flag,
        "--extra"
            | "-e"
            | "--package"
            | "-p"
            | "--python"
            | "--directory"
            | "--project"
            | "--env-file"
            | "--with"
            | "--with-requirements"
            | "--from"
            | "--allow-insecure-host"
    )
}

fn is_uvx_flag_with_arg(flag: &str) -> bool {
    if flag.contains('=') {
        return false;
    }
    matches!(
        flag,
        "--from" | "--with" | "--python" | "--package" | "-p" | "-e"
    )
}

fn is_python_binary(tok: &str) -> bool {
    if tok == "python" || tok == "python3" {
        return true;
    }
    if let Some(rest) = tok.strip_prefix("python3.") {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    false
}

fn unwrap_wrappers(tokens: &[String]) -> &[String] {
    let mut idx = 0;
    while idx < tokens.len() {
        let tok = unquote(&tokens[idx]);

        // 1. Env assignments like FOO=1, BAR="hello"
        if is_env_assignment(tok) {
            idx += 1;
            continue;
        }

        // env command: env [flags] [VAR=VAL] ...
        if tok == "env" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t == "-i" || t.starts_with("-u") || is_env_assignment(t) {
                    idx += 1;
                } else {
                    break;
                }
            }
            continue;
        }

        // 2. xargs [flags]
        if tok == "xargs" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if matches!(t, "-I" | "-n" | "-L" | "-s" | "-E" | "-d" | "-a") {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 3. uv run [flags]
        if tok == "uv" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if is_uv_flag_with_arg(t) {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 4. uvx [--from X] [flags]
        if tok == "uvx" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if is_uvx_flag_with_arg(t) {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 5. poetry run [flags]
        if tok == "poetry" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if matches!(t, "-C" | "--directory") {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 6. pipenv run [flags]
        if tok == "pipenv" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 7. pdm run [flags]
        if tok == "pdm" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 8. npx [flags]
        if tok == "npx" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if matches!(t, "-p" | "--package") {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 9. pnpm exec [flags]
        if tok == "pnpm" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "exec" {
            idx += 2;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 10. bunx [flags]
        if tok == "bunx" {
            idx += 1;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 11. python -m / python3 -m
        if is_python_binary(tok) {
            let mut m_idx = idx + 1;
            while m_idx < tokens.len() {
                let t = unquote(&tokens[m_idx]);
                if t == "-m" {
                    break;
                }
                if t.starts_with('-') {
                    m_idx += 1;
                } else {
                    break;
                }
            }
            if m_idx < tokens.len() && unquote(&tokens[m_idx]) == "-m" && m_idx + 1 < tokens.len() {
                idx = m_idx + 1;
                continue;
            }
        }

        break;
    }
    &tokens[idx..]
}

fn is_pnpm_builtin(s: &str) -> bool {
    matches!(
        s,
        "install"
            | "i"
            | "add"
            | "remove"
            | "rm"
            | "update"
            | "up"
            | "link"
            | "unlink"
            | "import"
            | "rebuild"
            | "prune"
            | "fetch"
            | "publish"
            | "pack"
            | "exec"
            | "dlx"
            | "create"
            | "init"
    )
}

fn is_yarn_builtin(s: &str) -> bool {
    matches!(
        s,
        "install"
            | "add"
            | "remove"
            | "upgrade"
            | "config"
            | "create"
            | "cache"
            | "link"
            | "unlink"
            | "publish"
            | "pack"
            | "init"
            | "dlx"
            | "node"
    )
}

fn extract_check_key_from_tokens(tokens: &[String]) -> Option<CheckKey> {
    if tokens.is_empty() {
        return None;
    }
    let cmd = unquote(&tokens[0]);
    match cmd {
        "make" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    if (t == "-C" || t == "-f") && i + 1 < tokens.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                } else if is_env_assignment(t) {
                    i += 1;
                } else {
                    return Some(CheckKey::Make(t.to_string()));
                }
            }
            None
        }
        "cargo" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('+') || t.starts_with('-') {
                    i += 1;
                } else {
                    return Some(CheckKey::Cargo(t.to_string()));
                }
            }
            None
        }
        "go" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else {
                    return Some(CheckKey::Go(t.to_string()));
                }
            }
            None
        }
        "npm" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "test" || t == "t" {
                    return Some(CheckKey::Npm("test".to_string()));
                } else if t == "run" || t == "run-script" {
                    i += 1;
                    while i < tokens.len() {
                        let script = unquote(&tokens[i]);
                        if script.starts_with('-') {
                            i += 1;
                        } else {
                            return Some(CheckKey::Npm(script.to_string()));
                        }
                    }
                    return None;
                } else {
                    return None;
                }
            }
            None
        }
        "pnpm" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "test" || t == "t" {
                    return Some(CheckKey::Pnpm("test".to_string()));
                } else if t == "run" {
                    i += 1;
                    while i < tokens.len() {
                        let script = unquote(&tokens[i]);
                        if script.starts_with('-') {
                            i += 1;
                        } else {
                            return Some(CheckKey::Pnpm(script.to_string()));
                        }
                    }
                    return None;
                } else if !is_pnpm_builtin(t) {
                    return Some(CheckKey::Pnpm(t.to_string()));
                } else {
                    return None;
                }
            }
            None
        }
        "yarn" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "run" {
                    i += 1;
                    while i < tokens.len() {
                        let script = unquote(&tokens[i]);
                        if script.starts_with('-') {
                            i += 1;
                        } else {
                            return Some(CheckKey::Yarn(script.to_string()));
                        }
                    }
                    return None;
                } else if !is_yarn_builtin(t) {
                    return Some(CheckKey::Yarn(t.to_string()));
                } else {
                    return None;
                }
            }
            None
        }
        "ruff" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "check" || t == "format" {
                    return Some(CheckKey::Ruff(t.to_string()));
                } else {
                    return None;
                }
            }
            None
        }
        "pytest" | "pre-commit" | "pyright" | "mypy" | "eslint" | "vitest" | "jest" | "tsc"
        | "tox" | "nox" | "black" | "isort" => Some(CheckKey::Tool(cmd.to_string())),
        _ => None,
    }
}

pub fn extract_check_keys(line: &str) -> Vec<CheckKey> {
    let mut keys = Vec::new();
    for part in split_pipeline_and_chain(line) {
        let tokens = tokenize_cmd(&part);
        let unwrapped = unwrap_wrappers(&tokens);
        if let Some(key) = extract_check_key_from_tokens(unwrapped) {
            keys.push(key);
        }
    }
    keys
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn contains_word_phrase(haystack: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let actual_pos = start + pos;
        let end_pos = actual_pos + needle.len();

        let before_ok = if actual_pos == 0 {
            true
        } else {
            let prev_char = haystack[..actual_pos].chars().next_back().unwrap();
            !is_identifier_char(prev_char)
        };

        let after_ok = if end_pos == haystack.len() {
            true
        } else {
            let next_char = haystack[end_pos..].chars().next().unwrap();
            !is_identifier_char(next_char)
        };

        if before_ok && after_ok {
            return true;
        }
        start = actual_pos + 1;
    }
    false
}

fn blyte_toml_comment_covers(blyte_toml_content: &str, key: &CheckKey) -> bool {
    blyte_toml_content.lines().any(|line| {
        let trimmed = line.trim();
        let Some(after_hash) = trimmed.strip_prefix('#') else {
            return false;
        };
        let comment = after_hash.trim();
        if contains_word_phrase(comment, &key.key_text()) {
            return true;
        }
        match key {
            CheckKey::Npm(s) if s == "test" => contains_word_phrase(comment, "npm test"),
            CheckKey::Pnpm(s) if s == "test" => {
                contains_word_phrase(comment, "pnpm test")
                    || contains_word_phrase(comment, "npm test")
            }
            CheckKey::Yarn(s) => contains_word_phrase(comment, &format!("yarn run {s}")),
            _ => false,
        }
    })
}

fn has_python_project_file(root: &Path) -> bool {
    let files = [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "tox.ini",
        "noxfile.py",
        "pytest.ini",
    ];
    files.iter().any(|f| root.join(f).exists())
}

pub fn is_project_file_missing(root: &Path, key: &CheckKey) -> bool {
    match key {
        CheckKey::Cargo(_) => !root.join("Cargo.toml").exists(),
        CheckKey::Go(_) => !root.join("go.mod").exists(),
        CheckKey::Npm(_) | CheckKey::Pnpm(_) | CheckKey::Yarn(_) => {
            !root.join("package.json").exists()
        }
        CheckKey::Make(target) => {
            let makefiles = ["Makefile", "makefile", "GNUmakefile"];
            for mf in makefiles {
                let path = root.join(mf);
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for line in content.lines() {
                            if line.starts_with('\t') || line.starts_with('#') {
                                continue;
                            }
                            let trimmed = line.trim_start();
                            if let Some(rest) = trimmed.strip_prefix(target.as_str()) {
                                if rest.starts_with(':') || rest.trim_start().starts_with(':') {
                                    return false;
                                }
                            }
                        }
                    }
                }
            }
            true
        }
        CheckKey::Ruff(_) => !has_python_project_file(root),
        CheckKey::Tool(tool) => match tool.as_str() {
            "pre-commit" => {
                !root.join(".pre-commit-config.yaml").exists()
                    && !root.join(".pre-commit-config.yml").exists()
            }
            "eslint" | "vitest" | "jest" | "tsc" => !root.join("package.json").exists(),
            "pytest" | "pyright" | "mypy" | "tox" | "nox" | "black" | "isort" => {
                !has_python_project_file(root)
            }
            _ => false,
        },
    }
}

/// Check whether a CI step is covered by the gate commands, marked in comments,
/// or inapplicable because its project file is missing at the repository root.
pub fn is_ci_step_covered(root: &Path, step: &str, gate: &str, blyte_toml_content: &str) -> bool {
    let step_keys = extract_check_keys(step);
    if step_keys.is_empty() {
        return true;
    }
    let gate_keys: Vec<CheckKey> = gate.lines().flat_map(extract_check_keys).collect();

    step_keys.iter().all(|key| {
        // 1. A gate line has the same key
        if gate_keys.iter().any(|gk| gk.matches(key)) {
            return true;
        }
        // 2. Any comment line in blyte.toml contains the step's key text, whatever it says
        if blyte_toml_comment_covers(blyte_toml_content, key) {
            return true;
        }
        // 3. The tool's project file is missing at the repository root
        if is_project_file_missing(root, key) {
            return true;
        }
        false
    })
}

/// Compute CI steps from repository workflows that are not covered by the gate, comments,
/// or missing project files.
pub fn missing_ci_steps(root: &Path, gate: &str, blyte_toml_content: &str) -> Vec<String> {
    let ci_steps = crate::init::drop_overlapping(crate::init::detect_ci(root));
    if ci_steps.is_empty() {
        return Vec::new();
    }
    let mut seen_keys = HashSet::new();
    let mut missing = Vec::new();

    for step in ci_steps {
        if is_ci_step_covered(root, &step, gate, blyte_toml_content) {
            continue;
        }
        let step_keys = extract_check_keys(&step);
        let mut is_new = false;
        for key in step_keys {
            if seen_keys.insert(key) {
                is_new = true;
            }
        }
        if is_new {
            missing.push(step);
        }
    }
    missing
}

/// Format the single-line warning for missing CI steps.
pub fn format_gate_drift_warning(missing: &[String]) -> Option<String> {
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "blyte: the gate is missing CI steps: {} (run blyte init --force to refresh blyte.toml)",
            missing.join(", ")
        ))
    }
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
