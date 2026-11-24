//! `blyte edit-start` and `blyte edit-finish` commands.

use blyte_agent::run::run_gate_steps;
use blyte_agent::shell::{LocalShell, Output, Shell};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cli::{EditFinishArgs, EditStartArgs};
use crate::dispatch::{CommitRecord, DispatchSummary};
use crate::report::{RunRecord, UsageRecord, SCHEMA_VERSION};

const TAIL_MAX_LINES: usize = 80;
const TAIL_MAX_BYTES: usize = 8192;
const ALREADY_PASSED_LOG: &str = "check skipped: tree already passed\n";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRecord {
    pub id: String,
    pub base: String,
    pub base_sha: String,
    pub started_at: u64,
}

#[derive(Debug, Serialize)]
pub struct EditStartOutput {
    pub id: String,
    pub path: String,
    pub base_sha: String,
}

#[derive(Debug, Serialize)]
pub struct EditGateFailedOutput {
    pub id: String,
    pub outcome: &'static str,
    pub failing_step: String,
    pub output_tail: String,
}

#[derive(Debug, Serialize)]
pub struct EditOutOfScopeOutput {
    pub id: String,
    pub outcome: &'static str,
    pub changed_files: Vec<String>,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct EditReviewFinding {
    pub file: String,
    pub line: Option<u64>,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct EditReviewFindingsOutput {
    pub id: String,
    pub outcome: &'static str,
    pub findings: Vec<EditReviewFinding>,
}

pub fn direct_edit_scope(changed: &[String]) -> Result<(), String> {
    if changed.len() <= 1 || changed.iter().all(|p| is_documentation_path(p)) {
        return Ok(());
    }
    Err(format!(
        "direct edits are for documentation or a change confined to one file; this edit changes {} files ({}): hand it to a run with blyte dispatch",
        changed.len(),
        changed.join(", ")
    ))
}

fn is_documentation_path(path: &str) -> bool {
    if path.starts_with("docs/") {
        return true;
    }
    let p = Path::new(path);
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        if matches!(
            ext.to_ascii_lowercase().as_str(),
            "md" | "mdx" | "rst" | "txt" | "adoc"
        ) {
            return true;
        }
    }
    p.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
        name.starts_with("README") || name.starts_with("CHANGELOG") || name.starts_with("LICENSE")
    })
}

struct StepOutput {
    command: String,
    output: String,
}

struct GateOutcome {
    elapsed: Duration,
    gate_attempts: u32,
    gate_wait_ms: u64,
    log_text: String,
}

struct EnvShell<S> {
    inner: S,
    env: BTreeMap<String, String>,
    steps: Mutex<Vec<StepOutput>>,
}

impl<S: Shell> Shell for EnvShell<S> {
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        let res = if self.env.is_empty() {
            self.inner.exec(command, cwd, timeout)
        } else {
            let mut script = String::new();
            for (k, v) in &self.env {
                let quoted = format!("'{}'", v.replace('\'', "'\\''"));
                script.push_str(&format!("export {k}={quoted}\n"));
            }
            script.push_str(command);
            self.inner.exec(&script, cwd, timeout)
        };
        let recorded = match &res {
            Ok(out) => format_step_output(out),
            Err(e) => format!("error: {e}\n"),
        };
        self.steps.lock().unwrap().push(StepOutput {
            command: command.to_string(),
            output: recorded,
        });
        res
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

fn format_step_output(out: &Output) -> String {
    let mut text = String::new();
    if !out.stdout.is_empty() {
        text.push_str(&out.stdout);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    if !out.stderr.is_empty() {
        text.push_str(&out.stderr);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    if text.is_empty() || out.exit_code != 0 {
        text.push_str(&format!("exit {}\n", out.exit_code));
    }
    text
}

fn format_gate_log(steps: &[StepOutput]) -> String {
    let mut log = String::new();
    for (idx, step) in steps.iter().enumerate() {
        if idx > 0 {
            log.push('\n');
        }
        log.push_str(&format!("$ {}\n", step.command));
        log.push_str(&step.output);
    }
    log
}

fn write_log(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("cannot write log {}: {e}", path.display()))
}

fn tail_output(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let mut joined = lines[start..].join("\n");
    if joined.len() > max_bytes {
        let mut cut = joined.len() - max_bytes;
        while cut < joined.len() && !joined.is_char_boundary(cut) {
            cut += 1;
        }
        joined = joined[cut..].to_string();
    }
    joined
}

fn now_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| format!("system clock before unix epoch: {e}"))
}

fn git_toplevel() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("failed to execute git: {e}"))?;
    if !out.status.success() {
        return Err("not a git repository".to_string());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn ensure_wt_excluded(root: &Path) {
    let common_dir_raw = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
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
}

fn branch_exists(root: &Path, branch: &str) -> Result<bool, String> {
    let branch_ref = format!("refs/heads/{branch}");
    let out = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &branch_ref])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git show-ref --verify --quiet {branch_ref}: {e}"))?;
    if out.status.success() {
        Ok(true)
    } else if out.status.code() == Some(1) {
        Ok(false)
    } else {
        Err(format!(
            "git show-ref --verify --quiet {branch_ref}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn cleanup_worktree_and_branch(root: &Path, worktree_dir: &Path, branch: &str) {
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_dir)
        .current_dir(root)
        .output();
    if worktree_dir.exists() {
        let _ = std::fs::remove_dir_all(worktree_dir);
    }
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(root)
        .output();
    let _ = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(root)
        .output();
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("failed to serialize {}: {e}", path.display()))?;
    json.push('\n');
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        format!(
            "failed to rename {} to {}: {e}",
            tmp_path.display(),
            path.display()
        )
    })
}

pub fn run_edit_start(args: &EditStartArgs) -> i32 {
    match run_edit_start_inner(args) {
        Ok(output) => {
            let json = serde_json::to_string_pretty(&output).unwrap();
            println!("{json}");
            0
        }
        Err(err) => {
            eprintln!("error: {err}");
            2
        }
    }
}

fn run_edit_start_inner(args: &EditStartArgs) -> Result<EditStartOutput, String> {
    crate::dispatch::validate_run_id(&args.id)?;
    if args.task.trim().is_empty() {
        return Err("--task must not be empty".to_string());
    }

    let start_dir = git_toplevel()?;
    let root = crate::clean::find_repo_root().map_err(|e| e.to_string())?;
    crate::registry::register(&root);

    let (repo_config, _) = crate::config::load_with_fallback(&start_dir)?
        .map(|(cfg, from_main)| (Some(cfg), from_main))
        .unwrap_or((None, false));
    let dispatch_cfg = repo_config.map(|c| c.dispatch).unwrap_or_default();

    let wt_parent = root.join(".wt");
    let worktree_dir = wt_parent.join(&args.id);
    let start_path = wt_parent.join(format!("{}.start.json", args.id));
    let edit_path = wt_parent.join(format!("{}.edit.json", args.id));
    let task_path = wt_parent.join(format!("{}.task.md", args.id));
    let branch = format!("wip/{}", args.id);

    if worktree_dir.exists() {
        return Err(format!(
            "worktree {} already exists",
            worktree_dir.display()
        ));
    }
    if start_path.exists() {
        return Err(format!(
            "start record {} already exists",
            start_path.display()
        ));
    }
    if edit_path.exists() {
        return Err(format!(
            "edit record {} already exists",
            edit_path.display()
        ));
    }
    if branch_exists(&root, &branch)? {
        return Err(format!("branch {branch} already exists"));
    }

    let base_sha = crate::dispatch::resolve_base_commit(&start_dir, &args.base)?;

    std::fs::create_dir_all(&wt_parent)
        .map_err(|e| format!("failed to create {}: {e}", wt_parent.display()))?;
    ensure_wt_excluded(&root);

    let add_out = Command::new("git")
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_dir)
        .arg(&base_sha)
        .current_dir(&root)
        .output()
        .map_err(|e| format!("failed to execute git worktree add: {e}"))?;
    if !add_out.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add_out.stderr).trim()
        ));
    }

    if let Some(ref setup_cmd) = dispatch_cfg.setup {
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
            cleanup_worktree_and_branch(&root, &worktree_dir, &branch);
            return Err(format!("setup failed: {msg}"));
        }
    }

    if let Err(e) = std::fs::write(&task_path, &args.task) {
        cleanup_worktree_and_branch(&root, &worktree_dir, &branch);
        return Err(format!("failed to write {}: {e}", task_path.display()));
    }

    let started_at = now_secs()?;
    let edit_record = EditRecord {
        id: args.id.clone(),
        base: args.base.clone(),
        base_sha: base_sha.clone(),
        started_at,
    };
    if let Err(e) = write_json_atomic(&edit_path, &edit_record) {
        let _ = std::fs::remove_file(&task_path);
        cleanup_worktree_and_branch(&root, &worktree_dir, &branch);
        return Err(e);
    }

    let abs_path = std::fs::canonicalize(&worktree_dir).unwrap_or(worktree_dir);
    Ok(EditStartOutput {
        id: args.id.clone(),
        path: abs_path.display().to_string(),
        base_sha,
    })
}

pub fn run_edit_finish(args: &EditFinishArgs) -> i32 {
    let start_dir = match git_toplevel() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let root = match crate::clean::find_repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: getting current executable: {e}");
            return 2;
        }
    };
    run_edit_finish_in(&root, &start_dir, &current_exe, args, None)
}

pub fn run_edit_finish_in(
    root: &Path,
    start_dir: &Path,
    blyte_exe: &Path,
    args: &EditFinishArgs,
    after_start_record: Option<&dyn Fn() -> Result<(), String>>,
) -> i32 {
    if let Err(e) = crate::dispatch::validate_run_id(&args.id) {
        eprintln!("error: {e}");
        return 2;
    }
    if let Some(ref req) = args.request {
        if let Err(e) = crate::dispatch::validate_request_id(req) {
            eprintln!("error: {e}");
            return 2;
        }
    }

    let wt_parent = root.join(".wt");
    let edit_path = wt_parent.join(format!("{}.edit.json", args.id));
    let summary_path = wt_parent.join(format!("{}.json", args.id));
    let start_path = wt_parent.join(format!("{}.start.json", args.id));
    let task_path = wt_parent.join(format!("{}.task.md", args.id));
    let log_path = wt_parent.join(format!("{}.log", args.id));
    let report_path = wt_parent.join(format!("{}.report.json", args.id));
    let worktree_dir = wt_parent.join(&args.id);
    let branch = format!("wip/{}", args.id);

    if !edit_path.exists() {
        eprintln!("error: missing edit record {}", edit_path.display());
        return 2;
    }
    if summary_path.exists() {
        eprintln!(
            "error: edit {} has already finished ({})",
            args.id,
            summary_path.display()
        );
        return 2;
    }

    let edit_raw = match std::fs::read_to_string(&edit_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read {}: {e}", edit_path.display());
            return 2;
        }
    };
    let edit: EditRecord = match serde_json::from_str(&edit_raw) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to parse {}: {e}", edit_path.display());
            return 2;
        }
    };

    if !worktree_dir.exists() {
        eprintln!("error: missing worktree {}", worktree_dir.display());
        return 2;
    }

    if let Err(e) = verify_worktree_git_pointer(root, &worktree_dir, &args.id) {
        eprintln!("error: {e}");
        return 2;
    }

    let task_text = match std::fs::read_to_string(&task_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "error: failed to read task file {}: {e}",
                task_path.display()
            );
            return 2;
        }
    };

    if let Some(ref msg) = args.message {
        if msg.trim().is_empty() {
            eprintln!("error: --message must not be empty");
            return 2;
        }
    }

    match crate::passed_trees::worktree_is_clean(&worktree_dir) {
        Ok(true) => {}
        Ok(false) => match args.message.as_deref() {
            Some(msg) => {
                if let Err(e) = commit_worktree_changes(&worktree_dir, msg) {
                    eprintln!("error: {e}");
                    return 2;
                }
            }
            None => {
                eprintln!(
                    "error: worktree {} has uncommitted or untracked changes; pass --message to commit them",
                    worktree_dir.display()
                );
                return 2;
            }
        },
        Err(e) => {
            eprintln!("error: failed to check worktree status: {e}");
            return 2;
        }
    }

    let commits = match collect_commits(&worktree_dir, &edit.base_sha) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if commits.is_empty() {
        eprintln!(
            "error: branch {branch} has no commits beyond base {}",
            edit.base_sha
        );
        return 2;
    }

    let changed_files = match collect_changed_files(&worktree_dir, &edit.base_sha) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    if let Err(error) = direct_edit_scope(&changed_files) {
        let out = EditOutOfScopeOutput {
            id: args.id.clone(),
            outcome: "out_of_scope",
            changed_files,
            error,
        };
        let json = serde_json::to_string_pretty(&out).unwrap();
        println!("{json}");
        return 2;
    }

    let (repo_config, from_main) = match crate::config::load_with_fallback(start_dir) {
        Ok(Some((cfg, from_main))) => (Some(cfg), from_main),
        Ok(None) => (None, false),
        Err(e) => {
            eprintln!("error: {e}");
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
    let configured_gate = match dispatch_cfg.gate {
        Some(ref g) if !g.trim().is_empty() => g.clone(),
        Some(_) => {
            eprintln!("error: the gate in blyte.toml is empty");
            return 2;
        }
        None => {
            eprintln!("error: no gate configured in blyte.toml");
            return 2;
        }
    };
    let gate_source = if from_main {
        "blyte.toml (main checkout)"
    } else {
        "blyte.toml"
    };

    let full_gate = if comments == crate::config::CommentMode::Gate {
        crate::dispatch::gate_with_comment_step(
            &configured_gate,
            &crate::dispatch::comment_step(blyte_exe, &edit.base_sha),
        )
    } else {
        configured_gate.clone()
    };

    let passed_key = crate::passed_trees::check_key(&configured_gate, &dispatch_cfg.env);
    let head_tree = match crate::passed_trees::tree_of(&worktree_dir, "HEAD") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let now = crate::passed_trees::now_secs();
    let already_passed = match crate::passed_trees::has_passed(root, &head_tree, &passed_key, now) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let gate_outcome = if already_passed {
        GateOutcome {
            elapsed: Duration::ZERO,
            gate_attempts: 0,
            gate_wait_ms: 0,
            log_text: ALREADY_PASSED_LOG.to_string(),
        }
    } else {
        let shell = EnvShell {
            inner: LocalShell::new(),
            env: dispatch_cfg.env.clone(),
            steps: Mutex::new(Vec::new()),
        };
        let mut deadline = None;
        let started = Instant::now();
        let gate_result = run_gate_steps(
            &shell,
            &full_gate,
            &worktree_dir,
            &[],
            &mut deadline,
            |_| {},
            &Instant::now,
        );
        let elapsed = started.elapsed();
        let log_text = format_gate_log(&shell.steps.lock().unwrap());
        if !gate_result.passed {
            if let Err(e) = write_log(&log_path, &log_text) {
                eprintln!("error: {e}");
                return 2;
            }
            let (failing_step, raw_output) = match gate_result.failures.first() {
                Some(f) => (f.step.clone(), f.output.as_str()),
                None => (full_gate.clone(), gate_result.output.as_str()),
            };
            let failed_out = EditGateFailedOutput {
                id: args.id.clone(),
                outcome: "gate_failed",
                failing_step,
                output_tail: tail_output(raw_output, TAIL_MAX_LINES, TAIL_MAX_BYTES),
            };
            let json = serde_json::to_string_pretty(&failed_out).unwrap();
            println!("{json}");
            return 1;
        }
        match crate::passed_trees::worktree_is_clean(&worktree_dir) {
            Ok(true) => {
                if let Err(e) = crate::passed_trees::record_passed(root, &head_tree, &passed_key) {
                    eprintln!("error: could not record passed tree: {e}");
                    return 2;
                }
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("error: failed to check worktree status after gate: {e}");
                return 2;
            }
        }
        GateOutcome {
            elapsed,
            gate_attempts: 1,
            gate_wait_ms: gate_result.wait_ms,
            log_text,
        }
    };

    if let Err(e) = write_log(&log_path, &gate_outcome.log_text) {
        eprintln!("error: {e}");
        return 2;
    }

    let gate_elapsed_ms = gate_outcome.elapsed.as_millis() as u64;
    let (gate_ms, gate_turns, gate_wait_ms, gate_seconds) = if gate_outcome.gate_attempts > 0 {
        (
            vec![gate_elapsed_ms],
            vec![0],
            vec![gate_outcome.gate_wait_ms],
            gate_elapsed_ms as f64 / 1000.0,
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new(), 0.0)
    };
    let report_record = RunRecord {
        schema_version: SCHEMA_VERSION,
        harness_build: env!("BLYTE_BUILD").into(),
        outcome: "verified".into(),
        error: None,
        final_message: Some(task_text.clone()),
        model_requested: None,
        model_id: None,
        effort: None,
        context: String::new(),
        gated: true,
        gate_source: gate_source.to_string(),
        gate_left_out: Vec::new(),
        container: None,
        container_workdir: String::new(),
        sandboxed: false,
        sandbox: "off".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        withheld_env: Vec::new(),
        duration_seconds: gate_outcome.elapsed.as_secs_f64(),
        turns: 0,
        empty_turns: 0,
        truncated_turns: 0,
        read_loop: false,
        peak_read_repeats: 0,
        fold_thrash: false,
        stalled: false,
        big_read_turns: 0,
        tool_calls: 0,
        gate_attempts: gate_outcome.gate_attempts,
        rollbacks: 0,
        provider_retries: 0,
        cost_usd: Some(0.0),
        max_cost_usd: None,
        cost_capped: false,
        fold_budget: None,
        turn_usage: Vec::new(),
        usage: UsageRecord::default(),
        gate_ms,
        gate_turns,
        gate_wait_ms,
        gate_failures: Vec::new(),
        tool_counts: BTreeMap::new(),
        model_seconds: 0.0,
        tools_seconds: 0.0,
        gate_seconds,
        quote_problems: None,
        claim_check: None,
        claim_check_skipped: None,
        base_check_passed: None,
        base_check_seconds: None,
    };
    if let Err(e) = crate::report::write(&report_path, &report_record) {
        eprintln!("error: {e}");
        return 2;
    }

    let seconds = gate_outcome.elapsed.as_secs();
    let gate_attempts = gate_outcome.gate_attempts;

    let comments_str = match comments {
        crate::config::CommentMode::Gate => "gate",
        crate::config::CommentMode::Review => "review",
        crate::config::CommentMode::Off => "off",
    };
    let env_names: Vec<&str> = dispatch_cfg.env.keys().map(String::as_str).collect();
    let session = crate::usage_log::session_from(
        std::env::var("BLYTE_SESSION").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    );
    let mut start_json = serde_json::json!({
        "id": args.id,
        "pid": std::process::id(),
        "started_at": edit.started_at,
        "task_file": task_path.display().to_string(),
        "gate": full_gate,
        "tags": Vec::<String>::new(),
        "env": env_names,
        "max_turns": 0,
        "max_seconds": 0,
        "plan": serde_json::Value::Null,
        "plan_task": serde_json::Value::Null,
        "request": args.request,
        "base": edit.base,
        "base_sha": edit.base_sha,
        "base_check": "off",
        "comments": comments_str,
        "kind": "edit",
    });
    if let Some(s) = session {
        start_json["session"] = serde_json::json!(s);
    }

    if let Err(e) = crate::dispatch::write_start_record(&start_path, &start_json) {
        eprintln!(
            "error: could not write start record {}: {e}",
            start_path.display()
        );
        return 2;
    }

    if let Some(hook) = after_start_record {
        if let Err(e) = hook() {
            eprintln!("error: {e}");
            return 2;
        }
    }

    let review = match crate::queue_actions::evaluate_verified_run_with(
        root,
        &args.id,
        true,
        crate::model_review::run,
    ) {
        Ok(mr) => mr,
        Err(e) => {
            eprintln!(
                "warning: post-edit review check failed for {}: {e}",
                args.id
            );
            None
        }
    };

    let fixround_path = crate::fix_round::record_path(&wt_parent, &args.id);
    let existing_fixround = match crate::fix_round::load_record(&fixround_path) {
        Ok(rec) => rec,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let needs_fix_round = crate::fix_round::should_run(false, "verified", true, review.as_ref())
        || review
            .as_ref()
            .is_some_and(crate::model_review::ModelReviewResult::has_findings);
    if existing_fixround.is_none() && needs_fix_round {
        let mr = review.as_ref().unwrap();
        let pre_round_sha = match git_head_sha(&worktree_dir) {
            Ok(sha) => sha,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let fallbacks_sent = if crate::fix_round::confirmed_count(mr) > 0 {
            match crate::fix_round::collect_confirmed_fallbacks(root, &args.id, &worktree_dir, mr) {
                Ok(fbs) => fbs,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            }
        } else {
            Vec::new()
        };
        let started_at = match now_secs() {
            Ok(ts) => ts,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let started_record = crate::fix_round::FixRoundRecord::Started {
            pre_round_sha,
            fallbacks_sent,
            started_at,
        };
        if let Err(e) = crate::fix_round::write_record(&fixround_path, &started_record) {
            eprintln!("error: {e}");
            return 2;
        }
        let findings = collect_review_findings(mr);
        let findings_out = EditReviewFindingsOutput {
            id: args.id.clone(),
            outcome: "review_findings",
            findings,
        };
        let json = serde_json::to_string_pretty(&findings_out).unwrap();
        println!("{json}");
        return 1;
    }

    let summary = DispatchSummary {
        id: args.id.clone(),
        worktree: worktree_dir.display().to_string(),
        branch,
        outcome: "verified",
        exit: 0,
        error: None,
        commits,
        changed_files,
        final_message: Some(task_text),
        turns: 0,
        gate_attempts,
        gate_failures: Vec::new(),
        cost_usd: Some(0.0),
        model: None,
        seconds,
        sandbox: "none".to_string(),
        fix_round: None,
        log: log_path.display().to_string(),
        report: report_path.display().to_string(),
        gate_source,
        research_report: None,
        base_check_seconds: None,
        warnings: Vec::new(),
    };

    if let Err(e) = write_json_atomic(&summary_path, &summary) {
        eprintln!("error: {e}");
        return 2;
    }

    let notify_outcome = crate::surface_notify::notify_runs_changed(root, &args.id);
    crate::surface_notify::report(&notify_outcome, &mut std::io::stderr());

    let json = serde_json::to_string_pretty(&summary).unwrap();
    println!("{json}");
    0
}

fn git_common_dir(dir: &Path) -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to execute git rev-parse --git-common-dir: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse --git-common-dir failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return Err("git rev-parse --git-common-dir returned empty output".to_string());
    }
    let path = PathBuf::from(raw);
    Ok(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

fn verify_worktree_git_pointer(
    main_checkout: &Path,
    worktree_dir: &Path,
    id: &str,
) -> Result<(), String> {
    let git_file = worktree_dir.join(".git");
    let meta = std::fs::symlink_metadata(&git_file).map_err(|e| {
        format!(
            "worktree git pointer {} cannot be inspected: {e}",
            git_file.display()
        )
    })?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        return Err(format!(
            "worktree git pointer {} is a symlink, expected a regular file",
            git_file.display()
        ));
    }
    if ft.is_dir() {
        return Err(format!(
            "worktree git pointer {} is a directory, expected a regular file",
            git_file.display()
        ));
    }
    if !ft.is_file() {
        return Err(format!(
            "worktree git pointer {} is not a regular file",
            git_file.display()
        ));
    }

    let content = std::fs::read_to_string(&git_file).map_err(|e| {
        format!(
            "worktree git pointer {} cannot be read: {e}",
            git_file.display()
        )
    })?;
    let trimmed = content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .unwrap_or(&content);
    let Some(pointed_str) = trimmed.strip_prefix("gitdir: ") else {
        return Err(format!(
            "worktree git pointer {} must be a single `gitdir: <path>` line",
            git_file.display()
        ));
    };
    if pointed_str.is_empty() || pointed_str.contains(['\n', '\r']) {
        return Err(format!(
            "worktree git pointer {} must be a single `gitdir: <path>` line",
            git_file.display()
        ));
    }

    let common_dir = git_common_dir(main_checkout).map_err(|e| {
        format!(
            "cannot resolve git common dir while verifying {}: {e}",
            git_file.display()
        )
    })?;
    let expected_worktree_gitdir = common_dir.join("worktrees").join(id);
    if !expected_worktree_gitdir.is_dir() {
        return Err(format!(
            "worktree git pointer {} expects worktree metadata directory {}, which does not exist",
            git_file.display(),
            expected_worktree_gitdir.display()
        ));
    }
    let expected_canon = std::fs::canonicalize(&expected_worktree_gitdir).map_err(|e| {
        format!(
            "cannot canonicalize {} while verifying {}: {e}",
            expected_worktree_gitdir.display(),
            git_file.display()
        )
    })?;

    let pointed_path = Path::new(pointed_str);
    let resolved_pointed = if pointed_path.is_absolute() {
        pointed_path.to_path_buf()
    } else {
        worktree_dir.join(pointed_path)
    };
    let pointed_canon = std::fs::canonicalize(&resolved_pointed).map_err(|e| {
        format!(
            "worktree git pointer {} points to {}, which cannot be resolved: {e}",
            git_file.display(),
            pointed_str
        )
    })?;
    if pointed_canon != expected_canon {
        return Err(format!(
            "worktree git pointer {} points to {}, expected {}",
            git_file.display(),
            pointed_canon.display(),
            expected_canon.display()
        ));
    }

    let backlink_file = expected_canon.join("gitdir");
    let backlink_content = std::fs::read_to_string(&backlink_file).map_err(|e| {
        format!(
            "worktree git pointer {} cannot be verified because {} cannot be read: {e}",
            git_file.display(),
            backlink_file.display()
        )
    })?;
    let backlink_trimmed = backlink_content
        .strip_suffix("\r\n")
        .or_else(|| backlink_content.strip_suffix('\n'))
        .unwrap_or(&backlink_content);
    if backlink_trimmed.is_empty() || backlink_trimmed.contains(['\n', '\r']) {
        return Err(format!(
            "worktree git pointer {} cannot be verified because {} is malformed",
            git_file.display(),
            backlink_file.display()
        ));
    }
    let backlink_path = Path::new(backlink_trimmed);
    let resolved_backlink = if backlink_path.is_absolute() {
        backlink_path.to_path_buf()
    } else {
        expected_canon.join(backlink_path)
    };
    let backlink_canon = std::fs::canonicalize(&resolved_backlink).map_err(|e| {
        format!(
            "worktree git pointer {} cannot be verified because {} points to {}, which cannot be resolved: {e}",
            git_file.display(),
            backlink_file.display(),
            backlink_trimmed
        )
    })?;
    let git_file_canon = std::fs::canonicalize(&git_file).map_err(|e| {
        format!(
            "cannot canonicalize worktree git pointer {}: {e}",
            git_file.display()
        )
    })?;
    if backlink_canon != git_file_canon {
        return Err(format!(
            "worktree git pointer {} is not the target of {} (points to {})",
            git_file.display(),
            backlink_file.display(),
            backlink_canon.display()
        ));
    }

    Ok(())
}

fn commit_worktree_changes(worktree_dir: &Path, message: &str) -> Result<(), String> {
    let add_out = Command::new("git")
        .current_dir(worktree_dir)
        .args(["add", "-A"])
        .output()
        .map_err(|e| format!("failed to run git add -A: {e}"))?;
    if !add_out.status.success() {
        return Err(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add_out.stderr).trim()
        ));
    }
    let commit_out = Command::new("git")
        .current_dir(worktree_dir)
        .args(["commit", "--no-gpg-sign", "-m", message])
        .output()
        .map_err(|e| format!("failed to run git commit: {e}"))?;
    if !commit_out.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit_out.stderr).trim()
        ));
    }
    Ok(())
}

fn collect_commits(worktree_dir: &Path, base_sha: &str) -> Result<Vec<CommitRecord>, String> {
    let out = Command::new("git")
        .current_dir(worktree_dir)
        .args([
            "log",
            "--reverse",
            "--format=%h%x09%s",
            &format!("{base_sha}..HEAD"),
        ])
        .output()
        .map_err(|e| format!("failed to run git log: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
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
    Ok(commits)
}

fn git_head_sha(worktree_dir: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(worktree_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("failed to run git rev-parse HEAD: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        return Err("git rev-parse HEAD returned empty output".to_string());
    }
    Ok(sha)
}

fn collect_review_findings(
    review: &crate::model_review::ModelReviewResult,
) -> Vec<EditReviewFinding> {
    let mut out = Vec::new();
    for c in &review.concerns {
        out.push(EditReviewFinding {
            file: c.file.clone(),
            line: c.line,
            text: c.text.clone(),
        });
    }
    for f in review.fallbacks.iter().filter(|f| f.verdict == "fallback") {
        out.push(EditReviewFinding {
            file: f.file.clone(),
            line: Some(f.line as u64),
            text: f.reason.clone(),
        });
    }
    out
}

fn collect_changed_files(worktree_dir: &Path, base_sha: &str) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .current_dir(worktree_dir)
        .args(["diff", "--name-only", &format!("{base_sha}..HEAD")])
        .output()
        .map_err(|e| format!("failed to run git diff: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut files = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            files.push(line.to_string());
        }
    }
    Ok(files)
}
