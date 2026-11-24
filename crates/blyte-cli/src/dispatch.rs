//! `blyte dispatch` and `blyte review` commands.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{DispatchArgs, ReviewArgs};

pub const DISPATCH_RULES: &str = r#"Rules:
- Stay within the task. If it cannot pass without a small change elsewhere, make the smallest one and say so.
- Write no comments in code: not to explain it, not to mark a section, not to note an assumption or a TODO. The check fails on any comment you add. If something needs saying, say it in your final report. Documentation comments a tool reads (///, //!, /** */) and tool directives (noqa, eslint-disable, SAFETY:) are the only exceptions; do not use them to get around this rule.
- Never delete, clean or modify anything outside the worktree, and never change the machine to make a check pass. If a check fails because of the environment (disk, network, a missing service), stop and say so.
- Never copy, link or use a dependency environment from outside the worktree (a .venv, node_modules, site-packages or a package-cache entry from the main checkout, another worktree, another repository or your home directory). If the environment cannot be built here, stop and say so.
- Find files with the glob tool and search file contents with the grep tool. Do not run `find`, `grep -r` or `rg` in the shell over `/`, your home directory, or any directory outside the worktree: they walk huge trees and are killed at the time limit.
- Do not push, open pull requests, or read secrets.
- Commit locally before you stop, with the exact commit message the task gives (or a conventional one if it gives none).
- When you believe the task is done and committed, stop calling tools: the harness then runs the check. Before that, run the check's quick steps (formatting, lint, the tests for your change) yourself; the full run happens at the gate.
- Do not run the full test suite or the whole check yourself, not before you start and not while you work: the gate runs it when you stop, and each extra run costs minutes. Run the tests for the code you change.
- Read the code before you change it. If the task is already satisfied, or is smaller or larger than it says, do what is actually needed and say so at the top of your final report: what you found and what you did instead. Nobody has read the code for you first.
- Finish with a short report: what you changed, what you ran, and anything that deviated from the task."#;

/// Writes a start record as pretty JSON plus a newline, through a temporary file renamed
/// into place, so a concurrent reader never sees a partly written file.
pub fn write_start_record(
    path: &std::path::Path,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let mut json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    json.push('\n');
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, path)
}

struct StartRecordGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for StartRecordGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

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
- To quote code, do not retype it: put a line `<<quote /absolute/path:N-M>>` on its own in report.md (at most 200 lines) and the harness replaces it with those lines of the file. You need the line numbers, not a fresh read of the file.
- Write your findings to `report.md` in your working directory, then stop calling tools and reply with one line summarising them.
- The harness checks report.md: every cited `path:line` must exist, and a code block must be copied from the file and lines cited on the line before it (`path:N-M`). A code block you type yourself is checked the same way, so prefer the quote line. A code block with no citation fails unless it is a command or output (```bash, ```text) or is tagged ```proposed (code you propose or sketch, not in the repository). If the check fails you get the list of problems to fix.
- Start report.md early, after your first findings, and keep it updated as you go, so it always holds your best answer."#;

pub fn wrap_research(question: &str, root: &Path) -> String {
    format!(
        "READ-ONLY RESEARCH TASK. The code to research is under {}: read it there by absolute path.\n\n{}\n\n{}",
        root.display(),
        question.trim(),
        RESEARCH_RULES
    )
}

pub fn comment_step(exe: &std::path::Path, base_sha: &str) -> String {
    let escaped = exe.display().to_string().replace('\'', r"'\''");
    format!("'{escaped}' comments --base {base_sha}")
}

pub fn gate_with_comment_step(gate: &str, step: &str) -> String {
    if gate.lines().all(|l| l.trim().is_empty()) {
        gate.to_string()
    } else {
        format!("{step}\n{gate}")
    }
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

pub fn summary_error(outcome: &str, error: Option<&str>) -> Option<String> {
    let err = error?;
    if outcome == "blocked" {
        err.lines().next().map(String::from)
    } else {
        Some(err.to_string())
    }
}

/// Bounds a gate failure entry for `.wt/<id>.json`, keeping its first line and tail.
pub fn cut_summary_gate_failure(s: &str) -> String {
    const MAX_CHARS: usize = 1_000;
    const FIRST_LINE_MAX: usize = 200;

    let total_chars = s.chars().count();
    if total_chars <= MAX_CHARS {
        return s.to_string();
    }

    let first_line = s.lines().next().unwrap_or("");
    let head_end = first_line
        .char_indices()
        .nth(FIRST_LINE_MAX)
        .map(|(i, _)| i)
        .unwrap_or(first_line.len());
    let head = &first_line[..head_end];
    let head_chars = head.chars().count();

    let tail_budget = MAX_CHARS.saturating_sub(head_chars);
    let tail_start_byte = s
        .char_indices()
        .nth(total_chars.saturating_sub(tail_budget))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let tail_cand = &s[tail_start_byte..];
    let tail = if tail_start_byte > 0 && s.as_bytes()[tail_start_byte - 1] == b'\n' {
        tail_cand
    } else if let Some(idx) = tail_cand.find('\n') {
        let after = &tail_cand[idx + 1..];
        if after.trim_matches(['\r', '\n']).is_empty() {
            tail_cand
        } else {
            after
        }
    } else {
        tail_cand
    };
    let tail_chars = tail.chars().count();
    let omitted = total_chars.saturating_sub(head_chars + tail_chars);
    format!("{head}\n[... {omitted} characters left out ...]\n{tail}")
}

#[derive(Debug, Serialize)]
pub struct CommitRecord {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseCheckDecision {
    Run,
    Disabled,
    AlreadyPassed,
}

impl BaseCheckDecision {
    pub fn as_json(self) -> serde_json::Value {
        match self {
            Self::Run => serde_json::json!(true),
            Self::Disabled => serde_json::json!(false),
            Self::AlreadyPassed => serde_json::json!("already_passed"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn decide_base_check(
    root: &Path,
    worktree_dir: &Path,
    base_sha: &str,
    configured_gate: &str,
    env: &std::collections::BTreeMap<String, String>,
    research: bool,
    no_base_check: bool,
    config_base_check: Option<bool>,
    now: u64,
) -> Result<(BaseCheckDecision, Option<String>), String> {
    if research || no_base_check || config_base_check == Some(false) {
        return Ok((BaseCheckDecision::Disabled, None));
    }
    let tree = crate::passed_trees::tree_of(worktree_dir, base_sha)?;
    let key = crate::passed_trees::check_key(configured_gate, env);
    if crate::passed_trees::has_passed(root, &tree, &key, now)? {
        Ok((BaseCheckDecision::AlreadyPassed, Some(tree)))
    } else {
        Ok((BaseCheckDecision::Run, Some(tree)))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn record_passed_after_run(
    root: &Path,
    worktree_dir: &Path,
    research: bool,
    base_check_decision: BaseCheckDecision,
    base_tree: Option<&str>,
    key: Option<&str>,
    outcome: &str,
    base_check_passed: Option<bool>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if research {
        return warnings;
    }
    let Some(key) = key else {
        return warnings;
    };
    if base_check_decision == BaseCheckDecision::Run && base_check_passed == Some(true) {
        if let Some(tree) = base_tree {
            if let Err(err) = crate::passed_trees::record_passed(root, tree, key) {
                warnings.push(format!("could not record passed base tree: {err}"));
            }
        }
    }
    if outcome == "verified" {
        match crate::passed_trees::worktree_is_clean(worktree_dir) {
            Ok(true) => match crate::passed_trees::tree_of(worktree_dir, "HEAD") {
                Ok(head_tree) => {
                    if let Err(err) = crate::passed_trees::record_passed(root, &head_tree, key) {
                        warnings.push(format!("could not record passed tree: {err}"));
                    }
                }
                Err(err) => {
                    warnings.push(format!("could not resolve HEAD tree to record pass: {err}"));
                }
            },
            Ok(false) => {}
            Err(err) => {
                warnings.push(format!("could not check worktree cleanliness: {err}"));
            }
        }
    }
    warnings
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_check_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_round: Option<crate::fix_round::FixRoundSummary>,
}

struct ImplementerConfig<'a> {
    current_exe: &'a Path,
    worktree_dir: &'a Path,
    root: &'a Path,
    id: &'a str,
    gate: &'a str,
    env: &'a std::collections::BTreeMap<String, String>,
    model: Option<&'a str>,
    max_seconds: u64,
    max_turns: u32,
    max_cost: Option<f64>,
    fold_budget: Option<usize>,
    sandbox: &'a str,
    network: &'a str,
    config_allow_read: &'a [PathBuf],
    cli_allow_read: &'a [PathBuf],
    pass_env: &'a [String],
    research: bool,
    no_claim_check: bool,
    left_out: &'a [(String, String)],
}

struct PassReport {
    report_outcome: String,
    error: Option<String>,
    final_message: Option<String>,
    turns: u32,
    report_turns: Option<u64>,
    gate_attempts: u32,
    gate_failures: Vec<String>,
    cost_usd: Option<f64>,
    model: Option<String>,
    seconds: u64,
    sandbox: String,
    base_check_passed: Option<bool>,
    base_check_seconds: Option<f64>,
}

fn open_pass_log(log_path: &Path, append_header: Option<&str>) -> Result<std::fs::File, String> {
    match append_header {
        None => std::fs::File::create(log_path)
            .map_err(|e| format!("error creating log file {}: {e}", log_path.display())),
        Some(header) => {
            use std::io::Write;
            let needs_leading_nl = match std::fs::read(log_path) {
                Ok(bytes) => !bytes.is_empty() && !bytes.ends_with(b"\n"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => {
                    return Err(format!(
                        "error reading log file {}: {e}",
                        log_path.display()
                    ));
                }
            };
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
                .map_err(|e| format!("error opening log file {}: {e}", log_path.display()))?;
            if needs_leading_nl {
                writeln!(f)
                    .map_err(|e| format!("error writing log file {}: {e}", log_path.display()))?;
            }
            writeln!(f, "{header}")
                .map_err(|e| format!("error writing log file {}: {e}", log_path.display()))?;
            Ok(f)
        }
    }
}

fn run_implementer_pass<F>(
    cfg: &ImplementerConfig<'_>,
    log_path: &Path,
    append_log_header: Option<&str>,
    report_path: &Path,
    task_prompt: &str,
    base_check_dir: Option<&Path>,
    on_spawn: F,
) -> Result<(), String>
where
    F: FnOnce(u32),
{
    if report_path.exists() {
        std::fs::remove_file(report_path)
            .map_err(|e| format!("error removing {}: {e}", report_path.display()))?;
    }
    let log_file = open_pass_log(log_path, append_log_header)?;
    let log_file_err = log_file
        .try_clone()
        .map_err(|e| format!("error cloning log file handle: {e}"))?;

    let mut cmd = Command::new(cfg.current_exe);
    cmd.current_dir(cfg.worktree_dir);
    cmd.stdout(log_file);
    cmd.stderr(log_file_err);

    for (k, v) in cfg.env {
        cmd.env(k, v);
    }
    cmd.env("BLYTE_DISPATCH_ID", cfg.id);
    cmd.env_remove(blyte_agent::shell::MODEL_SHELL_ENV);

    cmd.arg("--gate").arg(cfg.gate);
    cmd.arg("--rollback=false");
    cmd.arg("--max-seconds").arg(cfg.max_seconds.to_string());
    cmd.arg("--max-turns").arg(cfg.max_turns.to_string());
    cmd.arg("--report").arg(report_path);

    if let Some(m) = cfg.model {
        cmd.arg("--model").arg(m);
    }
    if let Some(c) = cfg.max_cost {
        cmd.arg("--max-cost").arg(c.to_string());
    }
    if let Some(fb) = cfg.fold_budget {
        cmd.arg("--fold-budget").arg(fb.to_string());
    }
    cmd.arg("--sandbox").arg(cfg.sandbox);
    cmd.arg("--network").arg(cfg.network);
    for r in cfg.config_allow_read {
        cmd.arg("--allow-read").arg(r);
    }
    for r in cfg.cli_allow_read {
        cmd.arg("--allow-read").arg(r);
    }
    for p in cfg.pass_env {
        cmd.arg("--pass-env").arg(p);
    }
    for r in cfg.config_allow_read.iter().chain(cfg.cli_allow_read) {
        cmd.arg("--read-root").arg(r);
    }
    if cfg.research {
        cmd.arg("--read-root").arg(cfg.root);
        cmd.arg("--no-gate-slot");
        cmd.arg("--research");
        if !cfg.no_claim_check {
            cmd.arg("--claim-check");
        }
    } else if let Some(base_wt_dir) = base_check_dir {
        cmd.arg("--base-check");
        cmd.arg("--base-check-dir").arg(base_wt_dir);
    }

    cmd.arg("--");
    cmd.arg(task_prompt);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("error spawning child process: {e}"))?;
    let pid = child.id();
    on_spawn(pid);
    if let Err(msg) = crate::keep_awake::hold(pid) {
        eprintln!("blyte: warning: {msg}; the machine may sleep during this run");
    }

    let _ = child.wait();

    if !cfg.left_out.is_empty() {
        if let Ok(report_str) = std::fs::read_to_string(report_path) {
            if let Ok(mut report_val) = serde_json::from_str::<serde_json::Value>(&report_str) {
                let formatted: Vec<String> = cfg
                    .left_out
                    .iter()
                    .map(|(cmd_name, reason)| format!("{cmd_name} ({reason})"))
                    .collect();
                report_val["gate_left_out"] = serde_json::json!(formatted);
                if let Ok(updated) = serde_json::to_string_pretty(&report_val) {
                    let _ = std::fs::write(report_path, updated + "\n");
                }
            }
        }
    }

    Ok(())
}

fn parse_pass_report(report_path: &Path, default_sandbox: &str) -> Result<PassReport, String> {
    let raw = std::fs::read_to_string(report_path)
        .map_err(|e| format!("missing report {}: {e}", report_path.display()))?;
    let rec: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("unreadable report JSON {}: {e}", report_path.display()))?;

    let gate_failures = match rec.get("gate_failures").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(cut_summary_gate_failure)
            .collect(),
        None => Vec::new(),
    };
    let raw_turns = rec.get("turns").and_then(|v| v.as_u64());
    let rec_outcome = rec
        .get("outcome")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing \"outcome\" in {}", report_path.display()))?;
    let turns = match raw_turns {
        Some(t) => t as u32,
        None => 0,
    };
    let gate_attempts = match rec.get("gate_attempts").and_then(|v| v.as_u64()) {
        Some(n) => n as u32,
        None => 0,
    };
    let seconds = match rec.get("duration_seconds").and_then(|v| v.as_f64()) {
        Some(d) => d.round() as u64,
        None => 0,
    };
    let sandbox = match rec.get("sandbox").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => default_sandbox.to_string(),
    };

    Ok(PassReport {
        report_outcome: rec_outcome.to_string(),
        error: summary_error(rec_outcome, rec.get("error").and_then(|v| v.as_str())),
        final_message: rec
            .get("final_message")
            .and_then(|v| v.as_str())
            .map(String::from),
        turns,
        report_turns: raw_turns,
        gate_attempts,
        gate_failures,
        cost_usd: rec.get("cost_usd").and_then(|v| v.as_f64()),
        model: rec
            .get("model_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        seconds,
        sandbox,
        base_check_passed: rec.get("base_check_passed").and_then(|v| v.as_bool()),
        base_check_seconds: rec.get("base_check_seconds").and_then(|v| v.as_f64()),
    })
}

fn pass_report_from_error(err_msg: String, default_sandbox: &str) -> PassReport {
    PassReport {
        report_outcome: "error".to_string(),
        error: Some(err_msg),
        final_message: None,
        turns: 0,
        report_turns: None,
        gate_attempts: 0,
        gate_failures: Vec::new(),
        cost_usd: None,
        model: None,
        seconds: 0,
        sandbox: default_sandbox.to_string(),
        base_check_passed: None,
        base_check_seconds: None,
    }
}

fn git_head_sha(worktree_dir: &Path) -> Result<String, String> {
    let wt = worktree_dir
        .to_str()
        .ok_or_else(|| format!("non-UTF-8 worktree path {}", worktree_dir.display()))?;
    let out = Command::new("git")
        .args(["-C", wt, "rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("failed to execute git rev-parse HEAD in {wt}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed in {wt}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_reset_hard(worktree_dir: &Path, sha: &str) -> Result<(), String> {
    let wt = worktree_dir
        .to_str()
        .ok_or_else(|| format!("non-UTF-8 worktree path {}", worktree_dir.display()))?;
    let out = Command::new("git")
        .args(["-C", wt, "reset", "--hard", sha])
        .output()
        .map_err(|e| format!("failed to run git reset --hard {sha} in {wt}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git reset --hard {sha} failed in {wt}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn git_commits_since(worktree_dir: &Path, base_sha: &str) -> Vec<CommitRecord> {
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
    commits
}

fn git_changed_files_since(worktree_dir: &Path, base_sha: &str) -> Vec<String> {
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
    changed_files
}

fn update_start_record_pid(start_path: &Path, pid: u32) {
    let raw = match std::fs::read_to_string(start_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("warning: cannot read {}: {e}", start_path.display());
            return;
        }
    };
    let mut val = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("warning: cannot parse {}: {e}", start_path.display());
            return;
        }
    };
    val["pid"] = serde_json::json!(pid);
    if let Err(e) = write_start_record(start_path, &val) {
        eprintln!("warning: cannot update {}: {e}", start_path.display());
    }
}

fn read_start_base_sha(start_path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(start_path)
        .map_err(|e| format!("cannot read {}: {e}", start_path.display()))?;
    let val: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse {}: {e}", start_path.display()))?;
    val.get("base_sha")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("missing \"base_sha\" in {}", start_path.display()))
}

fn read_start_task_content(root: &Path, start_path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(start_path)
        .map_err(|e| format!("cannot read {}: {e}", start_path.display()))?;
    let val: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse {}: {e}", start_path.display()))?;
    let task_file_str = val
        .get("task_file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing \"task_file\" in {}", start_path.display()))?;
    let task_path = Path::new(task_file_str);
    let resolved = if task_path.is_absolute() {
        task_path.to_path_buf()
    } else {
        root.join(task_path)
    };
    std::fs::read_to_string(&resolved)
        .map_err(|e| format!("cannot read task file {}: {e}", resolved.display()))
}

#[allow(clippy::too_many_arguments)]
fn build_error_summary(
    id: &str,
    worktree_dir: &Path,
    branch: String,
    err_msg: String,
    sandbox: String,
    log_path: &Path,
    report_path: &Path,
    gate_source: &'static str,
    warnings: Vec<String>,
) -> DispatchSummary {
    let outcome = "error";
    DispatchSummary {
        id: id.to_string(),
        worktree: worktree_dir.display().to_string(),
        branch,
        outcome,
        exit: exit_code(outcome),
        error: Some(err_msg),
        commits: Vec::new(),
        changed_files: Vec::new(),
        final_message: None,
        turns: 0,
        gate_attempts: 0,
        gate_failures: Vec::new(),
        cost_usd: None,
        model: None,
        seconds: 0,
        sandbox,
        log: log_path.display().to_string(),
        report: report_path.display().to_string(),
        gate_source,
        research_report: None,
        base_check_seconds: None,
        warnings,
        fix_round: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_fix_round(
    root: &Path,
    start_path: &Path,
    impl_cfg: &ImplementerConfig<'_>,
    log_path: &Path,
    fix_report_path: &Path,
    fixround_path: &Path,
    passed_key: Option<&str>,
    original_task: &str,
    pre_round_sha: String,
    fallbacks_sent: Vec<crate::fix_round::ConfirmedFallback>,
    round_started_at: u64,
    warnings: &mut Vec<String>,
) -> Result<(crate::fix_round::FixRoundSummary, PassReport), String> {
    let round_task = crate::fix_round::task_text(original_task, &fallbacks_sent);
    let mut round_prompt = wrap_task(&round_task);
    if let Some(qs) = quick_steps(impl_cfg.gate) {
        round_prompt.push_str("\n\n");
        round_prompt.push_str(&qs);
    }

    if fix_report_path.exists() {
        std::fs::remove_file(fix_report_path).map_err(|e| {
            format!(
                "cannot remove stale fix round report {}: {e}",
                fix_report_path.display()
            )
        })?;
    }

    run_implementer_pass(
        impl_cfg,
        log_path,
        Some("== fix round"),
        fix_report_path,
        &round_prompt,
        None,
        |pid| update_start_record_pid(start_path, pid),
    )?;

    let round_report = match parse_pass_report(fix_report_path, impl_cfg.sandbox) {
        Ok(r) => r,
        Err(err_msg) => pass_report_from_error(err_msg, impl_cfg.sandbox),
    };
    let round_commits = git_commits_since(impl_cfg.worktree_dir, &pre_round_sha);

    let (kept, round_outcome, done_commits, fallbacks_left) =
        if round_report.report_outcome == "verified" && !round_commits.is_empty() {
            let second_review = match crate::queue_actions::evaluate_verified_run(root, impl_cfg.id)
            {
                Ok(mr) => mr,
                Err(err) => {
                    eprintln!(
                        "warning: post-dispatch review check failed for {}: {err}",
                        impl_cfg.id
                    );
                    None
                }
            };
            let round_warnings = record_passed_after_run(
                root,
                impl_cfg.worktree_dir,
                false,
                BaseCheckDecision::Disabled,
                None,
                passed_key,
                "verified",
                None,
            );
            for w in &round_warnings {
                eprintln!("warning: {w}");
            }
            warnings.extend(round_warnings);
            let left = crate::fix_round::fallbacks_left(second_review.as_ref());
            let commit_shas = round_commits.into_iter().map(|c| c.sha).collect();
            (true, "verified".to_string(), commit_shas, left)
        } else {
            let round_outcome = if round_report.report_outcome == "verified" {
                "no_change".to_string()
            } else {
                match round_report.report_outcome.as_str() {
                    "gate_failed" => "gate_failed".to_string(),
                    "budget_exhausted" => "budget_exhausted".to_string(),
                    "blocked" => "blocked".to_string(),
                    _ => "error".to_string(),
                }
            };
            git_reset_hard(impl_cfg.worktree_dir, &pre_round_sha)?;
            let flag = crate::fix_round::failure_flag(&round_outcome);
            crate::fix_round::append_check_flag(root, impl_cfg.id, &flag)?;
            (false, round_outcome, Vec::new(), Some(fallbacks_sent.len()))
        };

    let fix_summary = crate::fix_round::FixRoundSummary {
        outcome: round_outcome,
        turns: round_report.turns,
        cost_usd: round_report.cost_usd,
        seconds: round_report.seconds,
        commits: done_commits,
        fallbacks_left,
        kept,
    };
    let done_record = crate::fix_round::FixRoundRecord::Done {
        pre_round_sha: Some(pre_round_sha),
        fallbacks_sent,
        started_at: Some(round_started_at),
        summary: fix_summary.clone(),
    };
    crate::fix_round::write_record(fixround_path, &done_record)?;
    Ok((fix_summary, round_report))
}

struct BaseWorktreeGuard {
    root: PathBuf,
    dir: PathBuf,
}

impl BaseWorktreeGuard {
    fn create(root: &Path, dir: PathBuf, base_sha: &str) -> Result<Self, String> {
        if dir.exists() {
            let _ = Command::new("git")
                .args([
                    "-C",
                    root.to_str().unwrap_or("."),
                    "worktree",
                    "remove",
                    "--force",
                ])
                .arg(&dir)
                .output();
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            let _ = Command::new("git")
                .args(["-C", root.to_str().unwrap_or("."), "worktree", "prune"])
                .output();
        }
        let out = Command::new("git")
            .args([
                "-C",
                root.to_str().unwrap_or("."),
                "worktree",
                "add",
                "--detach",
            ])
            .arg(&dir)
            .arg(base_sha)
            .output()
            .map_err(|e| format!("failed to run git worktree add --detach: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git worktree add --detach failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(Self {
            root: root.to_path_buf(),
            dir,
        })
    }
}

impl Drop for BaseWorktreeGuard {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args([
                "-C",
                self.root.to_str().unwrap_or("."),
                "worktree",
                "remove",
                "--force",
            ])
            .arg(&self.dir)
            .output();
    }
}

fn finish_summary(summary: &DispatchSummary, root: &Path) -> i32 {
    let wt_parent = root.join(".wt");
    let _ = std::fs::create_dir_all(&wt_parent);
    let json_str = serde_json::to_string_pretty(summary).unwrap();
    let summary_path = wt_parent.join(format!("{}.json", summary.id));
    let _ = std::fs::write(&summary_path, format!("{json_str}\n"));
    let notify_outcome = crate::surface_notify::notify_runs_changed(root, &summary.id);
    crate::surface_notify::report(&notify_outcome, &mut std::io::stderr());
    println!("{json_str}");
    summary.exit
}

pub fn branch_exists_message(root: &Path, branch: &str, id: &str) -> String {
    if crate::clean::is_branch_merged(root, branch) {
        format!(
            "error: branch {branch} already exists from an earlier run, and its work is already merged. Run `blyte clean` to remove it, or dispatch with a new ID."
        )
    } else {
        format!(
            "error: branch {branch} already exists from an earlier run and is not merged. Review it with `blyte review {id}`, delete it (`git branch -D {branch}`) if it is not needed, or dispatch with a new ID."
        )
    }
}

/// Source-state block for the research root and each read root, skipping duplicates.
fn research_source_block(
    root: &Path,
    config_allow_read: &[PathBuf],
    allow_read: &[PathBuf],
) -> String {
    let mut roots: Vec<PathBuf> = vec![root.to_path_buf()];
    for r in config_allow_read.iter().chain(allow_read) {
        if !roots.contains(r) {
            roots.push(r.clone());
        }
    }
    let sources: Vec<(PathBuf, crate::provenance::SourceState)> = roots
        .into_iter()
        .map(|p| {
            let state = crate::provenance::source_state(&p);
            (p, state)
        })
        .collect();
    crate::provenance::render(&sources)
}

/// Appends the source-state block to a copied report. Failure is reported, not fatal.
fn append_source_state(path: &Path, block: &str) {
    use std::io::Write;
    let result = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(format!("\n\n{block}").as_bytes()));
    if let Err(e) = result {
        eprintln!(
            "error: could not add the source state to {}: {e}",
            path.display()
        );
    }
}

pub fn validate_run_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("invalid run id: must not be empty".to_string());
    }
    if id.len() > 128 {
        return Err(format!(
            "invalid run id {:?}: must be at most 128 characters",
            id
        ));
    }
    if id.starts_with('.')
        || id.starts_with('-')
        || id.ends_with('.')
        || id.contains("..")
        || id.ends_with(".lock")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "invalid run id {:?}: only ASCII letters, digits, '.', '-' and '_' are allowed (no leading '.' or '-', no '..')",
            id
        ));
    }
    Ok(())
}

pub fn resolve_base_commit(start_dir: &Path, base: &str) -> Result<String, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", &format!("{base}^{{commit}}")])
        .current_dir(start_dir)
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => Err(format!("base {base} is not a commit")),
    }
}

pub fn validate_request_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("invalid --request value: must not be empty".to_string());
    }
    if id.len() > 64 {
        return Err(format!(
            "invalid --request value {:?}: must be at most 64 characters",
            id
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "invalid --request value {:?}: only ASCII letters, digits, '-' and '_' are allowed",
            id
        ));
    }
    Ok(())
}

pub fn run_dispatch(args: &DispatchArgs) -> i32 {
    if let Err(e) = validate_run_id(&args.id) {
        eprintln!("error: {e}");
        return 2;
    }
    if let Some(ref req) = args.request {
        if let Err(e) = validate_request_id(req) {
            eprintln!("error: {e}");
            return 2;
        }
    }
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
    let start_dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let root = match crate::clean::find_repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    crate::registry::register(&root);

    let (repo_config, from_main) = match crate::config::load_with_fallback(&start_dir) {
        Ok(Some((cfg, from_main))) => (Some(cfg), from_main),
        Ok(None) => (None, false),
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let dispatch_cfg = repo_config.map(|c| c.dispatch).unwrap_or_default();
    let comments = match crate::config::comment_mode(dispatch_cfg.comments.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let mut left_out: Vec<(String, String)> = Vec::new();
    let (gate, gate_source) = if args.research {
        ("test -s report.md".to_string(), "research")
    } else if let Some(ref g) = args.gate {
        (g.clone(), "flag")
    } else if let Some(ref g) = dispatch_cfg.gate {
        let toml_path = if from_main {
            root.join("blyte.toml")
        } else {
            start_dir.join("blyte.toml")
        };
        let toml_content = std::fs::read_to_string(&toml_path).unwrap_or_default();
        left_out = crate::gate_drift::left_out_steps(&toml_content);
        if g.trim().is_empty() {
            eprintln!(
                "error: the gate in blyte.toml is empty; every step was left out. Fix the steps blyte init named, or pass --gate."
            );
            for (cmd, reason) in &left_out {
                eprintln!("  {cmd}: {reason}");
            }
            return 2;
        }
        let missing = crate::gate_drift::missing_ci_steps(&root, g, &toml_content);
        if let Some(warning) = crate::gate_drift::format_gate_drift_warning(&missing) {
            eprintln!("{warning}");
        }
        if !left_out.is_empty() {
            let formatted: Vec<String> = left_out
                .iter()
                .map(|(cmd, reason)| format!("{cmd} ({reason})"))
                .collect();
            eprintln!(
                "blyte: the gate is partial; left out: {}",
                formatted.join("; ")
            );
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
    let task_md_path = wt_parent.join(format!("{}.task.md", args.id));
    let summary_path = wt_parent.join(format!("{}.json", args.id));
    let fixround_path = crate::fix_round::record_path(&wt_parent, &args.id);

    let existing_fixround = if args.research {
        None
    } else {
        match crate::fix_round::load_record(&fixround_path) {
            Ok(rec) => rec,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    };

    let resuming_fix_round = !args.research
        && !summary_path.exists()
        && matches!(
            existing_fixround,
            Some(crate::fix_round::FixRoundRecord::Started { .. })
        );

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
            _ if resuming_fix_round => {
                let start_path = wt_parent.join(format!("{}.start.json", args.id));
                match read_start_task_content(&root, &start_path) {
                    Ok(content) => (content, None),
                    Err(e) => {
                        eprintln!("error: {e}");
                        return 2;
                    }
                }
            }
            _ => {
                eprintln!("error: give a task file, or --plan with --plan-task");
                if args.research {
                    eprintln!(
                        "A research question needs these sections:\n\n{}",
                        crate::research_brief::TEMPLATE
                    );
                }
                return 2;
            }
        }
    };

    let brief_missing = args
        .research
        .then(|| crate::research_brief::missing_sections(&task_content));
    if let Some(ref missing) = brief_missing {
        if !args.no_brief_check {
            if !missing.is_empty() {
                eprintln!(
                    "blyte: warning: the research question has no section for: {}. Questions with all four run faster and fail less (the template: blyte dispatch {} --research); --no-brief-check hides this.",
                    missing.join(", "),
                    args.id
                );
            }
            let unverified = crate::research_brief::unverified_inputs(&task_content, &root);
            if !unverified.is_empty() {
                let paths: Vec<String> =
                    unverified.iter().map(|p| p.display().to_string()).collect();
                eprintln!(
                    "blyte: the question names a report that did not pass blyte's checks: {}. Use a verified report, or --no-brief-check to start anyway.",
                    paths.join(", ")
                );
                return 2;
            }
        }
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let main_checkout = root.clone();
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
        let mut readable = vec![start_dir.clone(), root.clone()];
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

    let task_file_recorded =
        recorded_task_file.unwrap_or_else(|| task_md_path.display().to_string());

    let mut replaced_earlier_run = false;
    if !args.research {
        let start_path = wt_parent.join(format!("{}.start.json", args.id));
        if start_path.exists() {
            let start_str = match std::fs::read_to_string(&start_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error reading {}: {e}", start_path.display());
                    return 2;
                }
            };
            let start_val: serde_json::Value = match serde_json::from_str(&start_str) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error parsing {}: {e}", start_path.display());
                    return 2;
                }
            };
            let Some(pid) = start_val.get("pid").and_then(|v| v.as_i64()) else {
                eprintln!("error: missing pid in {}", start_path.display());
                return 2;
            };
            let Some(started_at) = start_val.get("started_at").and_then(|v| v.as_u64()) else {
                eprintln!("error: missing started_at in {}", start_path.display());
                return 2;
            };
            if crate::clean::is_pid_alive(pid) {
                let finished_since_start = if summary_path.exists() {
                    let meta = match std::fs::metadata(&summary_path) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("error reading metadata of {}: {e}", summary_path.display());
                            return 2;
                        }
                    };
                    let mtime = match meta.modified() {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("error reading mtime of {}: {e}", summary_path.display());
                            return 2;
                        }
                    };
                    let mtime_secs = match mtime.duration_since(std::time::UNIX_EPOCH) {
                        Ok(d) => d.as_secs(),
                        Err(e) => {
                            eprintln!("error reading timestamp of {}: {e}", summary_path.display());
                            return 2;
                        }
                    };
                    mtime_secs >= started_at
                } else {
                    false
                };
                if !finished_since_start {
                    eprintln!(
                        "error: run {} is still running (pid {pid}). Wait for it to finish, or dispatch with a new ID.",
                        args.id
                    );
                    return 2;
                }
            }
        }

        if let Some(crate::fix_round::FixRoundRecord::Started {
            pre_round_sha,
            fallbacks_sent,
            started_at: round_started_at,
        }) = existing_fixround.clone()
        {
            if !summary_path.exists() {
                if !worktree_dir.exists() {
                    eprintln!("error: missing worktree {}", worktree_dir.display());
                    return 2;
                }
                let base_sha = match read_start_base_sha(&start_path) {
                    Ok(sha) => sha,
                    Err(e) => {
                        eprintln!("error: {e}");
                        return 2;
                    }
                };
                let report_path = wt_parent.join(format!("{}.report.json", args.id));
                let log_path = wt_parent.join(format!("{}.log", args.id));
                let fix_report_path = wt_parent.join(format!("{}.fix.report.json", args.id));
                let pass1 = match parse_pass_report(&report_path, &args.sandbox) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("error: {e}");
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
                let configured_gate = gate;
                let passed_key = Some(crate::passed_trees::check_key(
                    &configured_gate,
                    &dispatch_cfg.env,
                ));
                let effective_gate = if comments == crate::config::CommentMode::Gate {
                    gate_with_comment_step(&configured_gate, &comment_step(&current_exe, &base_sha))
                } else {
                    configured_gate
                };
                let mut warnings = Vec::new();
                if let Err(err_msg) = git_reset_hard(&worktree_dir, &pre_round_sha) {
                    let summary = build_error_summary(
                        &args.id,
                        &worktree_dir,
                        branch,
                        err_msg,
                        pass1.sandbox,
                        &log_path,
                        &report_path,
                        gate_source,
                        warnings,
                    );
                    return finish_summary(&summary, &root);
                }
                let impl_cfg = ImplementerConfig {
                    current_exe: &current_exe,
                    worktree_dir: &worktree_dir,
                    root: &root,
                    id: &args.id,
                    gate: &effective_gate,
                    env: &dispatch_cfg.env,
                    model: model.map(String::as_str),
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
                    left_out: &left_out,
                };
                let (fix_summary, round_report) = match execute_fix_round(
                    &root,
                    &start_path,
                    &impl_cfg,
                    &log_path,
                    &fix_report_path,
                    &fixround_path,
                    passed_key.as_deref(),
                    &task_content,
                    pre_round_sha,
                    fallbacks_sent,
                    round_started_at,
                    &mut warnings,
                ) {
                    Ok(res) => res,
                    Err(err_msg) => {
                        let summary = build_error_summary(
                            &args.id,
                            &worktree_dir,
                            branch,
                            err_msg,
                            pass1.sandbox,
                            &log_path,
                            &report_path,
                            gate_source,
                            warnings,
                        );
                        return finish_summary(&summary, &root);
                    }
                };
                let commits = git_commits_since(&worktree_dir, &base_sha);
                let changed_files = git_changed_files_since(&worktree_dir, &base_sha);
                let kept = fix_summary.kept;
                let summary = DispatchSummary {
                    id: args.id.clone(),
                    worktree: worktree_dir.display().to_string(),
                    branch,
                    outcome: "verified",
                    exit: exit_code("verified"),
                    error: pass1.error,
                    commits,
                    changed_files,
                    final_message: crate::fix_round::combined_final_message(
                        pass1.final_message,
                        round_report.final_message,
                        kept,
                    ),
                    turns: pass1.turns.saturating_add(round_report.turns),
                    gate_attempts: pass1.gate_attempts,
                    gate_failures: pass1.gate_failures,
                    cost_usd: crate::fix_round::sum_costs(pass1.cost_usd, round_report.cost_usd),
                    model: pass1.model.or(round_report.model),
                    seconds: pass1.seconds.saturating_add(round_report.seconds),
                    sandbox: pass1.sandbox,
                    log: log_path.display().to_string(),
                    report: report_path.display().to_string(),
                    gate_source,
                    research_report: None,
                    base_check_seconds: pass1.base_check_seconds,
                    warnings,
                    fix_round: Some(fix_summary),
                };
                return finish_summary(&summary, &root);
            }
        }

        let branch_ref = format!("refs/heads/{branch}");
        let branch_out = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", &branch_ref])
            .current_dir(&root)
            .output();
        let branch_exists = match branch_out {
            Ok(o) if o.status.success() => true,
            Ok(o) if o.status.code() == Some(1) => false,
            Ok(o) => {
                eprintln!(
                    "error: git show-ref --verify --quiet {branch_ref}: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: git show-ref --verify --quiet {branch_ref}: {e}");
                return 2;
            }
        };

        if worktree_dir.exists() || branch_exists {
            match crate::clean::remove_finished_run(&root, &args.id) {
                Ok(Some(reason)) => {
                    replaced_earlier_run = true;
                    let secs =
                        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                            Ok(d) => d.as_secs(),
                            Err(e) => {
                                eprintln!("error: system clock before unix epoch: {e}");
                                return 2;
                            }
                        };
                    let archive_rel = format!(".wt/archive/{}-{secs}", args.id);
                    let archive_dir = root.join(&archive_rel);
                    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
                        eprintln!("error: {}: {e}", archive_dir.display());
                        return 2;
                    }
                    for ext in [
                        "json",
                        "report.json",
                        "reported.json",
                        "start.json",
                        "verdict.json",
                        "checks.json",
                        "model-review.json",
                        "fixround.json",
                        "fix.report.json",
                        "log",
                    ] {
                        let file_name = format!("{}.{ext}", args.id);
                        let src = wt_parent.join(&file_name);
                        if src.exists() {
                            let dst = archive_dir.join(&file_name);
                            if let Err(e) = std::fs::rename(&src, &dst) {
                                eprintln!(
                                    "error: rename {} -> {}: {e}",
                                    src.display(),
                                    dst.display()
                                );
                                return 2;
                            }
                        }
                    }
                    eprintln!(
                        "dispatch: replaced the earlier run {} ({reason}); its records are in {archive_rel}/",
                        args.id
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("error: could not remove the earlier run {}: {e}", args.id);
                    return 2;
                }
            }
        }
    } else if worktree_dir.exists() {
        eprintln!("error: {} already exists", worktree_dir.display());
        return 2;
    }

    let fix_round_already_done = !replaced_earlier_run
        && matches!(
            existing_fixround,
            Some(crate::fix_round::FixRoundRecord::Done { .. })
        );

    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut tags = args.tags.clone();
    if args.research {
        tags.push("research".to_string());
    }
    let session = crate::usage_log::session_from(
        std::env::var("BLYTE_SESSION").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    );
    let start_path = wt_parent.join(format!("{}.start.json", args.id));
    let mut initial_start = serde_json::json!({
        "id": args.id,
        "pid": std::process::id(),
        "started_at": started_at,
        "starting": true,
        "tags": tags,
        "plan": args.plan,
        "plan_task": args.plan_task,
        "request": args.request,
        "base": args.base,
    });
    if let Some(ref s) = session {
        initial_start["session"] = serde_json::json!(s);
    }
    if let Some(ref missing) = brief_missing {
        initial_start["research"] = serde_json::json!(true);
        initial_start["brief_missing"] = serde_json::json!(missing);
    }
    if let Err(err) = std::fs::create_dir_all(&wt_parent)
        .and_then(|()| write_start_record(&start_path, &initial_start))
    {
        eprintln!(
            "error: could not write the start record {}: {err}",
            start_path.display()
        );
        return 2;
    }
    let mut start_guard = StartRecordGuard {
        path: start_path.clone(),
        armed: true,
    };

    if !args.research {
        crate::clean::spawn_background_clean(&root, Some(&args.id));
    }

    let base_commit: Option<String> = if args.research {
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
        None
    } else if !worktree_dir.exists() {
        let branch_exists = Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(&root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if branch_exists {
            eprintln!("{}", branch_exists_message(&root, &branch, &args.id));
            return 2;
        }
        let base_commit = match resolve_base_commit(&start_dir, &args.base) {
            Ok(sha) => sha,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let add_out = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch,
                worktree_dir.to_str().unwrap(),
                &base_commit,
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
        Some(base_commit)
    } else {
        let base_commit = match Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", args.base),
            ])
            .current_dir(&start_dir)
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Ok(o) => {
                eprintln!(
                    "error: git rev-parse {} failed: {}",
                    args.base,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: failed to execute git rev-parse: {e}");
                return 2;
            }
        };
        let wt_head = match Command::new("git")
            .args([
                "-C",
                worktree_dir.to_str().unwrap(),
                "rev-parse",
                "--verify",
                "HEAD^{commit}",
            ])
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Ok(o) => {
                eprintln!(
                    "error: git -C {} rev-parse HEAD failed: {}",
                    worktree_dir.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: failed to execute git rev-parse: {e}");
                return 2;
            }
        };
        let ancestor_out = Command::new("git")
            .args(["merge-base", "--is-ancestor", &base_commit, &wt_head])
            .current_dir(&root)
            .output();
        match ancestor_out {
            Ok(o) if o.status.success() => {}
            Ok(o) if o.status.code() == Some(1) => {
                let short_wt_head = &wt_head[..wt_head.len().min(7)];
                let short_base = &base_commit[..base_commit.len().min(7)];
                eprintln!(
                    "error: .wt/{id} is from an earlier run based on {short_wt_head}, which does not contain {short_base} ({base_name}). Run `blyte clean`, remove it with `git worktree remove .wt/{id}` and `git branch -D wip/{id}` if it is not needed, or dispatch with a new ID.",
                    id = args.id,
                    base_name = args.base,
                );
                return 2;
            }
            Ok(o) => {
                eprintln!(
                    "error: git merge-base --is-ancestor failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                return 2;
            }
            Err(e) => {
                eprintln!("error: failed to execute git merge-base --is-ancestor: {e}");
                return 2;
            }
        }
        Some(base_commit)
    };

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
                    base_check_seconds: None,
                    warnings: Vec::new(),
                    fix_round: None,
                };
                return finish_summary(&summary, &root);
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
    let summary_path = wt_parent.join(format!("{}.json", args.id));
    if summary_path.exists() {
        let _ = std::fs::remove_file(&summary_path);
    }
    let report_path = wt_parent.join(format!("{}.report.json", args.id));
    if report_path.exists() {
        let _ = std::fs::remove_file(&report_path);
    }
    let research_report_path = wt_parent.join(format!("{}.report.md", args.id));
    if research_report_path.exists() {
        let _ = std::fs::remove_file(&research_report_path);
    }
    let reported_marker_path = wt_parent.join(format!("{}.reported.json", args.id));
    if reported_marker_path.exists() {
        let _ = std::fs::remove_file(&reported_marker_path);
    }

    let configured_gate = gate;
    let passed_key = (!args.research)
        .then(|| crate::passed_trees::check_key(&configured_gate, &dispatch_cfg.env));
    let (base_check_decision, base_tree) = match decide_base_check(
        &root,
        &worktree_dir,
        &base_sha,
        &configured_gate,
        &dispatch_cfg.env,
        args.research,
        args.no_base_check,
        dispatch_cfg.base_check,
        crate::passed_trees::now_secs(),
    ) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if base_check_decision == BaseCheckDecision::AlreadyPassed {
        eprintln!("base check: this commit's content already passed");
    }

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error getting current executable: {e}");
            return 2;
        }
    };

    let gate = if !args.research && comments == crate::config::CommentMode::Gate {
        gate_with_comment_step(&configured_gate, &comment_step(&current_exe, &base_sha))
    } else {
        configured_gate
    };

    let mut base_wt_guard: Option<BaseWorktreeGuard> = None;
    let base_check_dir = if !args.research && base_check_decision == BaseCheckDecision::Run {
        let base_wt_dir = wt_parent.join(format!("{}.base", args.id));
        match BaseWorktreeGuard::create(&root, base_wt_dir.clone(), &base_sha) {
            Ok(guard) => {
                base_wt_guard = Some(guard);
                Some(base_wt_dir)
            }
            Err(e) => {
                eprintln!("error creating base check worktree: {e}");
                return 2;
            }
        }
    } else {
        None
    };

    let research_sources = args
        .research
        .then(|| research_source_block(&root, &config_allow_read, &args.allow_read));
    let task_prompt = if let Some(block) = &research_sources {
        wrap_research(&format!("{block}\n\n{task_content}"), &root)
    } else {
        let mut prompt = wrap_task(&task_content);
        if let Some(qs) = quick_steps(&gate) {
            prompt.push_str("\n\n");
            prompt.push_str(&qs);
        }
        prompt
    };

    let impl_cfg = ImplementerConfig {
        current_exe: &current_exe,
        worktree_dir: &worktree_dir,
        root: &root,
        id: &args.id,
        gate: &gate,
        env: &dispatch_cfg.env,
        model: model.map(String::as_str),
        max_seconds,
        max_turns,
        max_cost,
        fold_budget,
        sandbox: &args.sandbox,
        network,
        config_allow_read: &config_allow_read,
        cli_allow_read: &args.allow_read,
        pass_env: &args.pass_env,
        research: args.research,
        no_claim_check: args.no_claim_check,
        left_out: &left_out,
    };

    let comments_str = if args.research {
        "off"
    } else {
        match comments {
            crate::config::CommentMode::Gate => "gate",
            crate::config::CommentMode::Review => "review",
            crate::config::CommentMode::Off => "off",
        }
    };
    let env_names: Vec<&str> = dispatch_cfg.env.keys().map(String::as_str).collect();
    if let Err(e) = run_implementer_pass(
        &impl_cfg,
        &log_path,
        None,
        &report_path,
        &task_prompt,
        base_check_dir.as_deref(),
        |pid| {
            start_guard.armed = false;
            let mut start_json = serde_json::json!({
                "id": args.id,
                "pid": pid,
                "started_at": started_at,
                "task_file": task_file_recorded,
                "gate": gate,
                "tags": tags,
                "env": env_names,
                "max_turns": max_turns,
                "max_seconds": max_seconds,
                "plan": args.plan,
                "plan_task": args.plan_task,
                "request": args.request,
                "base": args.base,
                "base_sha": base_commit,
                "base_check": base_check_decision.as_json(),
                "comments": comments_str,
            });
            if let Some(ref session) = session {
                start_json["session"] = serde_json::json!(session);
            }
            if let Some(ref missing) = brief_missing {
                start_json["research"] = serde_json::json!(true);
                start_json["brief_missing"] = serde_json::json!(missing);
            }
            if !left_out.is_empty() {
                start_json["gate_left_out"] = serde_json::json!(left_out
                    .iter()
                    .map(|(command, reason)| serde_json::json!({
                        "command": command,
                        "reason": reason,
                    }))
                    .collect::<Vec<_>>());
            }
            if let Err(err) = write_start_record(&start_path, &start_json) {
                eprintln!(
                    "error: could not write the start record {}: {err}",
                    start_path.display()
                );
            }
        },
    ) {
        eprintln!("{e}");
        return 2;
    }
    drop(base_wt_guard);

    let scratch_report = worktree_dir.join("report.md");
    if args.research {
        let is_non_empty = std::fs::metadata(&scratch_report)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if is_non_empty {
            let _ = std::fs::copy(&scratch_report, &research_report_path);
            if let Some(block) = &research_sources {
                append_source_state(&research_report_path, block);
            }
            if let Some(ref report_to) = args.report_to {
                if let Some(parent) = report_to.parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                let _ = std::fs::copy(&scratch_report, report_to);
                if let Some(block) = &research_sources {
                    append_source_state(report_to, block);
                }
            }
        }
    }

    // 5. Read report, commits and changed files
    let mut commits = git_commits_since(&worktree_dir, &base_sha);
    let mut changed_files = git_changed_files_since(&worktree_dir, &base_sha);

    let pass1 = match parse_pass_report(&report_path, &args.sandbox) {
        Ok(r) => r,
        Err(err_msg) => pass_report_from_error(err_msg, &args.sandbox),
    };

    let outcome = if args.research {
        match pass1.report_outcome.as_str() {
            "verified" => "verified",
            "gate_failed" => "gate_failed",
            "budget_exhausted" => "budget_exhausted",
            "blocked" => "blocked",
            _ => "error",
        }
    } else {
        dispatch_outcome(&pass1.report_outcome, commits.len())
    };
    let exit = exit_code(outcome);

    if args.research {
        if research_report_path.exists() {
            if let Err(err) = crate::provenance::mark_if_unverified(
                &research_report_path,
                outcome,
                pass1.report_turns,
            ) {
                eprintln!(
                    "error: could not mark {} as not verified: {err}",
                    research_report_path.display()
                );
            }
        }
        if let Some(ref report_to) = args.report_to {
            if report_to.exists() {
                if let Err(err) =
                    crate::provenance::mark_if_unverified(report_to, outcome, pass1.report_turns)
                {
                    eprintln!(
                        "error: could not mark {} as not verified: {err}",
                        report_to.display()
                    );
                }
            }
        }
    }

    let research_report = if args.research && research_report_path.exists() {
        Some(research_report_path.display().to_string())
    } else {
        None
    };

    let mut warnings = record_passed_after_run(
        &root,
        &worktree_dir,
        args.research,
        base_check_decision,
        base_tree.as_deref(),
        passed_key.as_deref(),
        outcome,
        pass1.base_check_passed,
    );
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    let mut final_message = pass1.final_message;
    let mut turns = pass1.turns;
    let mut cost_usd = pass1.cost_usd;
    let mut seconds = pass1.seconds;
    let mut summary_model = pass1.model;
    let mut fix_round_summary: Option<crate::fix_round::FixRoundSummary> = None;

    if outcome == "verified" && !args.research {
        let first_review = match crate::queue_actions::evaluate_verified_run(&root, &args.id) {
            Ok(mr) => mr,
            Err(err) => {
                eprintln!(
                    "warning: post-dispatch review check failed for {}: {err}",
                    args.id
                );
                None
            }
        };

        if !fix_round_already_done
            && crate::fix_round::should_run(
                args.research,
                outcome,
                dispatch_cfg.fix_round,
                first_review.as_ref(),
            )
        {
            let review_ref = first_review.as_ref().unwrap();
            let pre_round_sha = match git_head_sha(&worktree_dir) {
                Ok(sha) => sha,
                Err(err_msg) => {
                    let summary = build_error_summary(
                        &args.id,
                        &worktree_dir,
                        branch,
                        err_msg,
                        pass1.sandbox,
                        &log_path,
                        &report_path,
                        gate_source,
                        warnings,
                    );
                    return finish_summary(&summary, &root);
                }
            };
            let fallbacks_sent = match crate::fix_round::collect_confirmed_fallbacks(
                &root,
                &args.id,
                &worktree_dir,
                review_ref,
            ) {
                Ok(fbs) => fbs,
                Err(err_msg) => {
                    let summary = build_error_summary(
                        &args.id,
                        &worktree_dir,
                        branch,
                        err_msg,
                        pass1.sandbox,
                        &log_path,
                        &report_path,
                        gate_source,
                        warnings,
                    );
                    return finish_summary(&summary, &root);
                }
            };
            let round_started_at =
                match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_secs(),
                    Err(e) => {
                        let summary = build_error_summary(
                            &args.id,
                            &worktree_dir,
                            branch,
                            format!("system clock before UNIX_EPOCH: {e}"),
                            pass1.sandbox,
                            &log_path,
                            &report_path,
                            gate_source,
                            warnings,
                        );
                        return finish_summary(&summary, &root);
                    }
                };
            let started_record = crate::fix_round::FixRoundRecord::Started {
                pre_round_sha: pre_round_sha.clone(),
                fallbacks_sent: fallbacks_sent.clone(),
                started_at: round_started_at,
            };
            if let Err(err_msg) = crate::fix_round::write_record(&fixround_path, &started_record) {
                let summary = build_error_summary(
                    &args.id,
                    &worktree_dir,
                    branch,
                    err_msg,
                    pass1.sandbox,
                    &log_path,
                    &report_path,
                    gate_source,
                    warnings,
                );
                return finish_summary(&summary, &root);
            }

            let fix_report_path = wt_parent.join(format!("{}.fix.report.json", args.id));
            match execute_fix_round(
                &root,
                &start_path,
                &impl_cfg,
                &log_path,
                &fix_report_path,
                &fixround_path,
                passed_key.as_deref(),
                &task_content,
                pre_round_sha,
                fallbacks_sent,
                round_started_at,
                &mut warnings,
            ) {
                Ok((fix_sum, round_report)) => {
                    commits = git_commits_since(&worktree_dir, &base_sha);
                    changed_files = git_changed_files_since(&worktree_dir, &base_sha);
                    turns = turns.saturating_add(round_report.turns);
                    cost_usd = crate::fix_round::sum_costs(cost_usd, round_report.cost_usd);
                    seconds = seconds.saturating_add(round_report.seconds);
                    summary_model = summary_model.or(round_report.model);
                    final_message = crate::fix_round::combined_final_message(
                        final_message,
                        round_report.final_message,
                        fix_sum.kept,
                    );
                    fix_round_summary = Some(fix_sum);
                }
                Err(err_msg) => {
                    let summary = build_error_summary(
                        &args.id,
                        &worktree_dir,
                        branch,
                        err_msg,
                        pass1.sandbox,
                        &log_path,
                        &report_path,
                        gate_source,
                        warnings,
                    );
                    return finish_summary(&summary, &root);
                }
            }
        }
    }

    // 6. Print JSON summary
    let summary = DispatchSummary {
        id: args.id.clone(),
        worktree: worktree_dir.display().to_string(),
        branch,
        outcome,
        exit,
        error: pass1.error,
        commits,
        changed_files,
        final_message,
        turns,
        gate_attempts: pass1.gate_attempts,
        gate_failures: pass1.gate_failures,
        cost_usd,
        model: summary_model,
        seconds,
        sandbox: pass1.sandbox,
        log: log_path.display().to_string(),
        report: report_path.display().to_string(),
        gate_source,
        research_report,
        base_check_seconds: pass1.base_check_seconds,
        warnings,
        fix_round: fix_round_summary,
    };

    finish_summary(&summary, &root)
}

pub fn run_review(args: &ReviewArgs) -> i32 {
    let root = match crate::clean::find_repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    if args.notes.is_some() && args.verdict.is_none() {
        eprintln!("error: --notes requires --verdict");
        return 2;
    }

    if let Some(v) = args.verdict {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Err(e) = crate::queue_actions::record_verdict(
            &root,
            &args.id,
            v,
            args.notes.as_deref().unwrap_or(""),
            ts,
        ) {
            eprintln!("{e}");
            return 2;
        }
        println!("recorded verdict {} for {}", v.as_str(), args.id);
        return 0;
    }

    let mut data = match crate::queue_actions::compute_review(&root, &args.id, args.diff) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let stored_model_review = if args.model_review {
        None
    } else {
        match crate::queue_actions::read_stored_model_review(&root, &args.id) {
            Ok(mr) => mr,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    };
    if !args.model_review {
        data.checks
            .flags
            .extend(crate::fallback_check::flags_for_review(
                &data.fallbacks,
                stored_model_review.as_ref(),
            ));
        if let Some(ref mr) = stored_model_review {
            if let Some(flag) = mr.flag() {
                if !data
                    .checks
                    .flags
                    .iter()
                    .any(|f| f.starts_with("model review"))
                {
                    data.checks.flags.push(flag);
                }
            }
        }
    }

    if !args.model_review {
        if let Err(e) = crate::queue_actions::write_checks_file(
            &root,
            &args.id,
            &data.checks.flags,
            &data.fallbacks,
            stored_model_review.as_ref(),
        ) {
            eprintln!("{e}");
            return 2;
        }
    }

    let crate::queue_actions::ReviewComputation {
        wt,
        wt_exists,
        is_research: _,
        base,
        task_text: _,
        changed_files: _,
        commits_out,
        files_out,
        tree_out,
        diff_out,
        hcmds,
        reads,
        report_val,
        quote_problems,
        claim_check,
        claim_check_skipped,
        mut checks,
        fallbacks,
    } = data;

    println!("== checks");
    if checks.flags.is_empty() {
        println!("nothing flagged");
    } else {
        for flag in &checks.flags {
            println!("! {flag}");
        }
    }

    println!("== commits");
    if !wt_exists {
        println!("(worktree removed)");
    } else if !commits_out.is_empty() {
        println!("{commits_out}");
    }
    println!("== files");
    if !wt_exists {
        println!("(worktree removed)");
    } else if !files_out.is_empty() {
        println!("{files_out}");
    }
    println!("== tree");
    if !wt_exists {
        println!("(worktree removed)");
    } else if tree_out.is_empty() {
        println!("(clean)");
    } else {
        println!("{tree_out}");
    }
    println!("== report");
    if let Some(ref v) = report_val {
        if let Some(outcome) = v.get("outcome").and_then(|x| x.as_str()) {
            println!("outcome: {outcome}");
        }
        let final_message = v.get("final_message").and_then(|x| x.as_str());
        if let Some(err) = v
            .get("error")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            if !final_message.is_some_and(|m| m.contains(err)) {
                println!("error: {err}");
            }
        }
        if let Some(turns) = v.get("turns").and_then(|x| x.as_u64()) {
            println!("turns: {turns}");
        }
        if let Some(cost) = v.get("cost_usd").and_then(|x| x.as_f64()) {
            println!("cost_usd: {cost}");
        }
        if let Some(msg) = final_message {
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
        if let Some(ref cc) = claim_check {
            println!(
                "claim check: {} checked, {} contradicted, {} not enough evidence, {} parse errors, {} unchecked",
                cc.checked,
                cc.contradicted.len(),
                cc.not_enough_evidence,
                cc.parse_errors,
                cc.unchecked
            );
            for f in cc.contradicted.iter().take(5) {
                let cut: String = f.text.chars().take(200).collect();
                println!("  report.md:{}: {cut}", f.line);
                println!("    {}", f.reason);
            }
        }
        if let Some(ref skipped) = claim_check_skipped {
            println!("claim check skipped: {skipped}");
        }
    }

    if let Some(ref v) = report_val {
        if let Some(failures) = v.get("gate_failures").and_then(|x| x.as_array()) {
            let entries: Vec<&str> = failures.iter().filter_map(|x| x.as_str()).collect();
            if let Some(last) = entries.last() {
                println!("== check failures");
                if entries.len() > 1 {
                    println!("({} failed attempts; the last one)", entries.len());
                }
                println!("{last}");
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
        if !wt_exists {
            println!("(worktree removed)");
        } else if !diff_out.is_empty() {
            println!("{diff_out}");
        }
    }

    if args.model_review && wt_exists {
        let mr = crate::model_review::run(&root, &args.id, &base, &wt);
        crate::model_review::print_result(&mr);
        checks.flags.extend(crate::fallback_check::flags_for_review(
            &fallbacks,
            Some(&mr),
        ));
        if let Some(flag) = mr.flag() {
            checks.flags.push(flag);
        }
        if let Err(e) = crate::queue_actions::write_checks_file(
            &root,
            &args.id,
            &checks.flags,
            &fallbacks,
            Some(&mr),
        ) {
            eprintln!("{e}");
            return 2;
        }
    } else if args.model_review {
        checks
            .flags
            .extend(crate::fallback_check::flags_for_review(&fallbacks, None));
        if let Err(e) = crate::queue_actions::write_checks_file(
            &root,
            &args.id,
            &checks.flags,
            &fallbacks,
            None,
        ) {
            eprintln!("{e}");
            return 2;
        }
    } else if let Some(ref mr) = stored_model_review {
        crate::model_review::print_result(mr);
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
    names_outside && changes_something(cmd)
}

pub(crate) fn bash_commands(log: &str) -> Vec<String> {
    let cleaned = strip_ansi(log);
    let mut result = Vec::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("● bash ") {
            if let Some((cmd, _)) = rest.rsplit_once(" -> ") {
                let cmd = cmd.trim();
                if !cmd.is_empty() {
                    result.push(cmd.to_string());
                }
            }
        }
    }
    result
}

pub fn host_commands(log: &str) -> Vec<String> {
    let mut result = Vec::new();
    for cmd in bash_commands(log) {
        if is_host_command(&cmd) && !result.iter().any(|c| c == &cmd) {
            result.push(cmd);
            if result.len() == 20 {
                break;
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
    let root = match crate::clean::find_repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

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
