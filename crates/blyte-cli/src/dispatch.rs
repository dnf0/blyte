//! `blyte dispatch` and `blyte review` commands.

use serde::{Deserialize, Serialize};
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

    let (gate, gate_source) = if let Some(ref g) = args.gate {
        (g.clone(), "flag")
    } else if let Some(ref g) = dispatch_cfg.gate {
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
    let max_seconds = args
        .max_seconds
        .or(dispatch_cfg.max_seconds)
        .unwrap_or(3600);
    let max_turns = args.max_turns.or(dispatch_cfg.max_turns).unwrap_or(300);
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
    if report_path.exists() {
        let _ = std::fs::remove_file(&report_path);
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

    cmd.arg("--");
    let mut task_prompt = wrap_task(&task_content);
    if let Some(qs) = quick_steps(&gate) {
        task_prompt.push_str("\n\n");
        task_prompt.push_str(&qs);
    }
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
    let start_json = serde_json::json!({
        "id": args.id,
        "pid": child.id(),
        "started_at": started_at,
        "task_file": args.task_file.display().to_string(),
        "gate": gate,
        "tags": args.tags,
        "env": env_names,
    });
    let start_path = wt_parent.join(format!("{}.start.json", args.id));
    if let Ok(json_str) = serde_json::to_string_pretty(&start_json) {
        let _ = std::fs::write(&start_path, json_str + "\n");
    }

    let _ = child.wait();

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

    let log_file = root.join(".wt").join(format!("{}.log", args.id));
    if let Ok(log_content) = std::fs::read_to_string(&log_file) {
        let hcmds = host_commands(&log_content);
        if !hcmds.is_empty() {
            println!("== host commands (may have changed things outside the worktree)");
            for cmd in hcmds {
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
