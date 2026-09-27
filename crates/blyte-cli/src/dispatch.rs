//! `blyte dispatch` and `blyte review` commands.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

use crate::cli::{DispatchArgs, ReviewArgs};

pub const DISPATCH_RULES: &str = r#"Rules:
- Stay within the task. If it cannot pass without a small change elsewhere, make the smallest one and say so.
- Never delete, clean or modify anything outside the worktree, and never change the machine to make a check pass. If a check fails because of the environment (disk, network, a missing service), stop and say so.
- Do not push, open pull requests, or read secrets.
- Commit locally before you stop, with the exact commit message the task gives (or a conventional one if it gives none).
- When you believe the task is done and committed, stop calling tools: the harness then runs the check. Before that, run the check's quick steps (formatting, lint, the tests for your change) yourself; the full run happens at the gate.
- Finish with a short report: what you changed, what you ran, and anything that deviated from the task."#;

pub fn wrap_task(task: &str) -> String {
    format!(
        "You are implementing exactly one task. Your working directory is a git worktree on its own branch; work only there.\n\n{}\n\n{}",
        task.trim(),
        DISPATCH_RULES
    )
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
        "error" => "error",
        _ => "error",
    }
}

pub fn exit_code(outcome: &str) -> i32 {
    match outcome {
        "verified" => 0,
        "no_changes" => 4,
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

    let repo_config = match crate::config::load(&root) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let dispatch_cfg = repo_config.map(|c| c.dispatch).unwrap_or_default();

    let (gate, gate_source) = if let Some(ref g) = args.gate {
        (g.clone(), "flag")
    } else if let Some(ref g) = dispatch_cfg.gate {
        (g.clone(), "blyte.toml")
    } else {
        eprintln!("no gate: pass --gate, or run blyte init to write blyte.toml");
        return 2;
    };

    let model = args.model.as_ref().or(dispatch_cfg.model.as_ref());
    let max_seconds = args
        .max_seconds
        .or(dispatch_cfg.max_seconds)
        .unwrap_or(3600);
    let max_turns = args.max_turns.or(dispatch_cfg.max_turns).unwrap_or(300);
    let max_cost = args.max_cost_usd.or(dispatch_cfg.max_cost);
    let network = args
        .network
        .as_deref()
        .or(dispatch_cfg.network.as_deref())
        .unwrap_or("on");

    let wt_parent = root.join(".wt");
    let worktree_dir = wt_parent.join(&args.id);
    let branch = format!("wip/{}", args.id);

    if !worktree_dir.exists() {
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
    if let Some(fb) = args.fold_budget {
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

    cmd.arg("--");
    cmd.arg(wrap_task(&task_content));

    let _ = cmd.status();

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

    let outcome = dispatch_outcome(&report_outcome, commits.len());
    let exit = exit_code(outcome);

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
    };

    let json_str = serde_json::to_string_pretty(&summary).unwrap();
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
        _ => root_head_sha,
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
    if args.diff {
        println!("== diff");
        if !diff_out.is_empty() {
            println!("{diff_out}");
        }
    }

    0
}
