//! `blyte dashboard` local HTTP server and job collector (plan 2026-09-30).

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone)]
pub struct Sources {
    pub roots: Vec<PathBuf>,
    pub runs_dirs: Vec<PathBuf>,
    pub hours: u64,
}

#[derive(serde::Deserialize)]
struct DashboardConfig {
    roots: Option<Vec<String>>,
    runs_dirs: Option<Vec<String>>,
}

fn expand_home(path_str: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path_str.strip_prefix("~/") {
        home.join(rest)
    } else if path_str == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(path_str)
    }
}

impl Sources {
    pub fn from_args(args: &crate::cli::DashboardArgs) -> Self {
        Self::from_args_with_where(args).0
    }

    pub fn from_args_with_where(args: &crate::cli::DashboardArgs) -> (Self, String) {
        if !args.roots.is_empty() || !args.runs_dirs.is_empty() {
            return (
                Self {
                    roots: args.roots.clone(),
                    runs_dirs: args.runs_dirs.clone(),
                    hours: args.hours,
                },
                "--root/--runs-dir".to_string(),
            );
        }

        let mut roots = Vec::new();
        let mut runs_dirs = Vec::new();
        let mut dashboard_contributed = false;

        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let config_path = home.join(".config/blyte/dashboard.toml");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(cfg) = toml::from_str::<DashboardConfig>(&content) {
                    if let Some(r) = cfg.roots {
                        for p in r {
                            let expanded = expand_home(&p, &home);
                            if !roots.contains(&expanded) {
                                roots.push(expanded);
                                dashboard_contributed = true;
                            }
                        }
                    }
                    if let Some(d) = cfg.runs_dirs {
                        for p in d {
                            let expanded = expand_home(&p, &home);
                            if !runs_dirs.contains(&expanded) {
                                runs_dirs.push(expanded);
                                dashboard_contributed = true;
                            }
                        }
                    }
                }
            }
        }

        let mut dispatch_contributed = false;
        for reg in crate::registry::registered() {
            let already_listed = roots
                .iter()
                .any(|r| r == &reg || std::fs::canonicalize(r).map(|c| c == reg).unwrap_or(false));
            if !already_listed {
                roots.push(reg);
                dispatch_contributed = true;
            }
        }

        let mut parts = Vec::new();
        if dashboard_contributed {
            parts.push("dashboard.toml");
        }
        if dispatch_contributed {
            parts.push("dispatch history");
        }
        let where_from = parts.join(" and ");

        (
            Self {
                roots,
                runs_dirs,
                hours: args.hours,
            },
            where_from,
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub sha: String,
    pub subject: String,
}

#[derive(serde::Serialize, Debug, Clone)]
pub struct Job {
    pub key: String,
    pub id: String,
    pub repo: String,
    pub status: String,
    pub gate_running: bool,
    pub title: String,
    pub task: String,
    pub plan: Option<String>,
    pub now: String,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub elapsed_s: Option<u64>,
    pub cost_usd: Option<f64>,
    pub turns: u64,
    pub gate_attempts: u64,
    pub gate_failures: Vec<String>,
    pub model: Option<String>,
    pub last_tool: Option<String>,
    pub worktree: Option<String>,
    pub worktree_exists: Option<bool>,
    pub log: String,
    pub commits: Vec<CommitRecord>,
    pub changed_files: Vec<String>,
    pub tags: Vec<String>,
    pub verdict: Option<String>,
    pub problems: Vec<String>,
    pub root: Option<PathBuf>,
    pub plan_file: Option<String>,
    pub plan_task: Option<String>,
    pub request: Option<String>,
    pub task_file: Option<String>,
    #[serde(skip)]
    pub research: bool,
    #[serde(skip)]
    pub start_plan: Option<String>,
    #[serde(skip)]
    pub file_tags: Vec<String>,
    #[serde(skip)]
    pub verdict_notes: Option<String>,
    #[serde(skip)]
    pub verdict_ts: Option<u64>,
    #[serde(skip)]
    pub log_mtime: Option<u64>,
}

#[derive(serde::Serialize, Debug, Clone)]
pub struct Summary {
    pub running: usize,
    pub last_24h: usize,
    pub repos: usize,
    pub finished_24h: usize,
    pub passed_24h: usize,
    pub cost_24h_usd: f64,
}

#[derive(serde::Serialize, Debug, Clone)]
pub struct DashboardResponse {
    pub generated_at: u64,
    pub summary: Summary,
    pub jobs: Vec<Job>,
}

fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    let bytes = s.as_bytes();
    let Some(first_esc) = bytes.iter().position(|&b| b == 0x1b) else {
        return std::borrow::Cow::Borrowed(s);
    };

    let mut out = String::with_capacity(s.len());
    let mut seg_start = 0;
    let mut i = first_esc;

    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            out.push_str(&s[seg_start..i]);
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                if (0x40..=0x7e).contains(&b) {
                    i += 1;
                    break;
                }
                if b == b'\n' || b == 0x1b {
                    out.push(b as char);
                    i += 1;
                    break;
                }
                i += 1;
            }
            seg_start = i;
        } else {
            i += 1;
        }
        if let Some(rel) = bytes[i..].iter().position(|&b| b == 0x1b) {
            i += rel;
        } else {
            break;
        }
    }

    out.push_str(&s[seg_start..]);
    std::borrow::Cow::Owned(out)
}

fn extract_title(task: &str) -> String {
    for line in task.lines() {
        let trimmed = line.trim();
        if let Some(pos) = trimmed.find("Task: \"") {
            let rest = &trimmed[pos + "Task: \"".len()..];
            if let Some(end) = rest.find('"') {
                return rest[..end].chars().take(120).collect();
            }
        }
    }
    for line in task.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let text = trimmed.trim_start_matches('#').trim();
            if !text.is_empty() {
                return text.chars().take(120).collect();
            }
        }
    }
    for line in task.lines() {
        let trimmed = line.trim_start_matches(['#', ' ']).trim_end();
        if !trimmed.is_empty() {
            return trimmed.chars().take(120).collect();
        }
    }
    String::new()
}

fn job_title(task: &str, id: &str) -> String {
    let title = extract_title(task);
    if id.is_empty() {
        return title;
    }
    let p_colon = format!("Task {id}:");
    let p_space = format!("Task {id} ");
    let rest = if let Some(after) = title.strip_prefix(&p_colon) {
        after.trim()
    } else if let Some(after) = title.strip_prefix(&p_space) {
        after.trim()
    } else {
        return title;
    };
    if rest.is_empty() {
        title
    } else {
        rest.chars().take(120).collect()
    }
}

fn extract_plan(task: &str) -> Option<String> {
    for line in task.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Plan: ") {
            let plan = rest.trim();
            if !plan.is_empty() {
                return Some(plan.to_string());
            }
        }
    }
    None
}

fn extract_prompt_task_title(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let trimmed = line.trim();
        if let Some(pos) = trimmed.find("Task: \"") {
            let rest = &trimmed[pos + "Task: \"".len()..];
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn extract_first_task_id(text: &str) -> Option<String> {
    for line in text.lines() {
        let plain = plain_line(line);
        let mut idx = 0;
        while let Some(pos) = plain[idx..].find("Task") {
            let start = idx + pos;
            let after = &plain[start + 4..];
            if after.starts_with(' ') || after.starts_with('\t') {
                let trimmed = after.trim_start();
                let id: String = trimmed
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if !id.is_empty() {
                    return Some(format!("Task {id}"));
                }
            }
            idx = start + 4;
        }
    }
    None
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes > 0 {
        let after = &trimmed[hashes..];
        if after.is_empty() || after.starts_with(' ') || after.starts_with('\t') {
            return Some((hashes, after.trim()));
        }
    }
    None
}

fn extract_task_from_plan(plan_content: &str, title: &str) -> Option<String> {
    let mut target_heading = None;
    let mut offset = 0;

    for line in plan_content.split_inclusive('\n') {
        let line_len = line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some((level, text)) = parse_heading(trimmed) {
            if text == title {
                target_heading = Some((offset, level));
                break;
            }
        }
        offset += line_len;
    }

    let (start_offset, target_level) = target_heading?;

    // A `#` line inside a fenced code block (a shell comment, say) is not a heading.
    let mut end_offset = plan_content.len();
    let mut curr_offset = 0;
    let mut in_fence = false;
    for line in plan_content.split_inclusive('\n') {
        let line_len = line.len();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if curr_offset > start_offset && !in_fence {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Some((level, _)) = parse_heading(trimmed) {
                if level <= target_level {
                    end_offset = curr_offset;
                    break;
                }
            }
        }
        curr_offset += line_len;
    }

    Some(plan_content[start_offset..end_offset].to_string())
}

fn extract_now_from_final_message(msg: &str) -> String {
    let pick = msg
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .or_else(|| msg.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("");
    plain_line(pick.trim_start_matches(['#', ' ']))
        .chars()
        .take(200)
        .collect()
}

/// One line of markdown as plain text: no list marker, no `**`, `__` or backticks.
fn plain_line(line: &str) -> String {
    let mut l = line.trim();
    if let Some(rest) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
        l = rest;
    } else if let Some((n, rest)) = l.split_once(". ") {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            l = rest;
        }
    }
    l.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

struct LogInfo {
    turns: u64,
    last_tool: Option<String>,
    gate_attempts: u64,
    gate_running: bool,
    mtime: Option<u64>,
}

enum OptionalFile<T> {
    Missing,
    Ok(T),
    Err,
}

fn read_optional_string(path: &Path, problems: &mut Vec<String>) -> OptionalFile<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => OptionalFile::Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => OptionalFile::Missing,
        Err(e) => {
            problems.push(format!("{}: {e}", path.display()));
            OptionalFile::Err
        }
    }
}

fn read_optional_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    problems: &mut Vec<String>,
) -> OptionalFile<T> {
    let raw = match read_optional_string(path, problems) {
        OptionalFile::Missing => return OptionalFile::Missing,
        OptionalFile::Err => return OptionalFile::Err,
        OptionalFile::Ok(s) => s,
    };
    match serde_json::from_str(&raw) {
        Ok(v) => OptionalFile::Ok(v),
        Err(e) => {
            problems.push(format!("{}: {e}", path.display()));
            OptionalFile::Err
        }
    }
}

fn read_required_u64(
    report: &serde_json::Value,
    field: &str,
    path: &Path,
    problems: &mut Vec<String>,
) -> u64 {
    match report.get(field) {
        Some(v) => match v.as_u64() {
            Some(n) => n,
            None => {
                problems.push(format!(
                    "{}: invalid '{field}' field (expected non-negative integer)",
                    path.display()
                ));
                0
            }
        },
        None => {
            problems.push(format!(
                "{}: missing required field '{field}'",
                path.display()
            ));
            0
        }
    }
}

fn parse_log(path: &Path, problems: &mut Vec<String>) -> LogInfo {
    let mtime = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let raw = match read_optional_string(path, problems) {
        OptionalFile::Ok(s) => s,
        OptionalFile::Missing | OptionalFile::Err => String::new(),
    };
    let text = strip_ansi(&raw);

    let mut max_turn = 0u64;
    let mut last_tool = None;
    let mut gate_attempts = 0u64;
    let mut last_event_line: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(pos) = trimmed.find("[Turn ") {
            let rest = &trimmed[pos + "[Turn ".len()..];
            if let Some(end) = rest.find(']') {
                if let Ok(n) = rest[..end].trim().parse::<u64>() {
                    if n > max_turn {
                        max_turn = n;
                    }
                }
            }
        }

        if let Some(pos) = trimmed.find("● ") {
            let tool = trimmed[pos + "● ".len()..].trim();
            last_tool = Some(tool.chars().take(200).collect());
        }

        if trimmed.starts_with("✓ Gate passed") || trimmed.starts_with("✗ Gate failed") {
            gate_attempts += 1;
        }

        if trimmed.contains("[Turn ")
            || trimmed.contains("● ")
            || trimmed.starts_with("⏳ Gate running")
            || trimmed.starts_with("✓ Gate passed")
            || trimmed.starts_with("✗ Gate failed")
        {
            last_event_line = Some(trimmed.to_string());
        }
    }

    let gate_running = last_event_line
        .map(|l| l.starts_with("⏳ Gate running"))
        .unwrap_or(false);

    LogInfo {
        turns: max_turn,
        last_tool,
        gate_attempts,
        gate_running,
        mtime,
    }
}

fn file_mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn started_with_job(pid: i64, started_at: Option<u64>, now: u64) -> bool {
    match (get_process_etime(pid), started_at) {
        (Some(elapsed), Some(started_at)) => now.saturating_sub(elapsed) <= started_at + 60,
        _ => true,
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => match out.components().next_back() {
                Some(std::path::Component::Normal(_)) => {
                    out.pop();
                }
                Some(std::path::Component::RootDir | std::path::Component::Prefix(_)) => {}
                _ => out.push(comp.as_os_str()),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn git_common_dir(start: &Path) -> Option<PathBuf> {
    if !start.exists() {
        return None;
    }
    let resolved_start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::fs::canonicalize(start).ok()?
    };
    for dir in resolved_start.ancestors() {
        let dot_git = dir.join(".git");
        let Ok(meta) = std::fs::metadata(&dot_git) else {
            continue;
        };
        if meta.is_dir() {
            return Some(
                std::fs::canonicalize(&dot_git).unwrap_or_else(|_| normalize_path(&dot_git)),
            );
        }
        if meta.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let first_line = content.lines().next()?.trim();
            let raw_gitdir = first_line.strip_prefix("gitdir:")?.trim();
            if raw_gitdir.is_empty() {
                return None;
            }
            let gitdir_path = Path::new(raw_gitdir);
            let gitdir = if gitdir_path.is_absolute() {
                gitdir_path.to_path_buf()
            } else {
                dir.join(gitdir_path)
            };
            let commondir_file = gitdir.join("commondir");
            let common = if commondir_file.is_file() {
                let raw_common = std::fs::read_to_string(&commondir_file).ok()?;
                let trimmed = raw_common.trim();
                if trimmed.is_empty() {
                    gitdir
                } else {
                    let common_path = Path::new(trimmed);
                    if common_path.is_absolute() {
                        common_path.to_path_buf()
                    } else {
                        gitdir.join(common_path)
                    }
                }
            } else {
                gitdir
            };
            return Some(
                std::fs::canonicalize(&common).unwrap_or_else(|_| normalize_path(&common)),
            );
        }
    }
    None
}

pub fn parse_etime(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (days, time_str) = match s.split_once('-') {
        Some((d, rest)) => {
            let days = d.parse::<u64>().ok()?;
            (days, rest)
        }
        None => (0, s),
    };

    let parts: Vec<&str> = time_str.split(':').collect();
    match parts.len() {
        2 => {
            let m = parts[0].parse::<u64>().ok()?;
            let sec = parts[1].parse::<u64>().ok()?;
            Some(days * 86400 + m * 60 + sec)
        }
        3 => {
            let h = parts[0].parse::<u64>().ok()?;
            let m = parts[1].parse::<u64>().ok()?;
            let sec = parts[2].parse::<u64>().ok()?;
            Some(days * 86400 + h * 3600 + m * 60 + sec)
        }
        _ => None,
    }
}

fn get_process_etime(pid: i64) -> Option<u64> {
    // macOS: ask the kernel for the start time. `ps` is setuid there, so blyte's own sandbox
    // cannot run it, and this also saves a process per PID.
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let ret = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if ret == size && info.pbi_start_tvsec > 0 {
            let now = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            return Some(now.saturating_sub(info.pbi_start_tvsec));
        }
    }

    if let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "etime=", "-p", &pid.to_string()])
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            if let Some(sec) = parse_etime(&s) {
                return Some(sec);
            }
        }
    }

    None
}

struct RawJob {
    id: String,
    repo: String,
    root: Option<PathBuf>,
    task: String,
    task_file: Option<String>,
    pid: Option<i64>,
    started_at: Option<u64>,
    research: bool,
    start_plan: Option<String>,
    start_plan_task: Option<String>,
    request: Option<String>,
    file_tags: Vec<String>,
    log_path: PathBuf,
    report_path: PathBuf,
    tags_path: PathBuf,
    verdict_path: PathBuf,
    worktree: Option<String>,
    commits: Vec<CommitRecord>,
    changed_files: Vec<String>,
    summary_final_msg: Option<String>,
    is_runs_dir: bool,
    problems: Vec<String>,
    problem_mtime: Option<u64>,
}

pub fn repo_name(checkout: &Path) -> String {
    let from_git = git_common_dir(checkout).and_then(|common| {
        if common.file_name().and_then(|n| n.to_str()) == Some(".git") {
            common
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
        } else {
            None
        }
    });

    let raw = from_git.unwrap_or_else(|| {
        checkout
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_string())
            .or_else(|| {
                checkout
                    .canonicalize()
                    .ok()
                    .as_deref()
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    });

    if let Some((prefix, _)) = raw.split_once("--") {
        if !prefix.is_empty() {
            return prefix.to_string();
        }
    }
    raw
}

pub fn collect_jobs(src: &Sources, now: u64) -> Vec<Job> {
    let cutoff = now.saturating_sub(src.hours.saturating_mul(3600));
    let mut raw_jobs = Vec::new();
    let mut seen_keys = HashSet::new();

    // 1. Dispatch layout
    let mut wt_dirs = Vec::new();
    for root in &src.roots {
        let r_wt = root.join(".wt");
        if r_wt.is_dir() && !wt_dirs.contains(&r_wt) {
            wt_dirs.push(r_wt);
        }
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let c_wt = path.join(".wt");
                    if c_wt.is_dir() && !wt_dirs.contains(&c_wt) {
                        wt_dirs.push(c_wt);
                    }
                }
            }
        }
    }

    for wt_dir in &wt_dirs {
        let repo = wt_dir
            .parent()
            .map(repo_name)
            .unwrap_or_else(|| "unknown".to_string());

        if let Ok(entries) = std::fs::read_dir(wt_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let id = if let Some(id) = fname.strip_suffix(".start.json") {
                    Some(id)
                } else if let Some(id) = fname.strip_suffix(".report.json") {
                    (!id.ends_with(".fix") && !id.is_empty()).then_some(id)
                } else {
                    None
                };
                if let Some(id) = id {
                    let id = id.to_string();
                    let key = format!("{repo}/{id}");
                    if !seen_keys.insert(key) {
                        continue;
                    }

                    let report_path = wt_dir.join(format!("{id}.report.json"));
                    let log_path = wt_dir.join(format!("{id}.log"));
                    let report_mtime = file_mtime(&report_path);
                    let log_mtime = file_mtime(&log_path);
                    if report_mtime.is_some_and(|m| m < cutoff)
                        && log_mtime.is_none_or(|m| m < cutoff)
                    {
                        continue;
                    }

                    let mut problems = Vec::new();
                    let start_path = wt_dir.join(format!("{id}.start.json"));
                    let start_val =
                        match read_optional_json::<serde_json::Value>(&start_path, &mut problems) {
                            OptionalFile::Ok(v) => Some(v),
                            OptionalFile::Missing | OptionalFile::Err => None,
                        };
                    let start_mtime = file_mtime(&start_path);
                    let pid = start_val.as_ref().and_then(|v| v["pid"].as_i64());

                    if report_mtime.is_none() && log_mtime.is_none_or(|m| m < cutoff) {
                        let has_recent_problem =
                            !problems.is_empty() && start_mtime.is_some_and(|m| m >= cutoff);
                        let is_alive = pid.is_some_and(|p| {
                            p > 0 && unsafe { libc::kill(p as libc::pid_t, 0) == 0 }
                        });
                        if !is_alive && !has_recent_problem {
                            continue;
                        }
                    }

                    let started_at = start_val.as_ref().and_then(|v| v["started_at"].as_u64());
                    let research = start_val
                        .as_ref()
                        .and_then(|v| v.get("research"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let start_plan = start_val
                        .as_ref()
                        .and_then(|v| v["plan"].as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty());
                    let start_plan_task = start_val
                        .as_ref()
                        .and_then(|v| v["plan_task"].as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty());
                    let request = start_val
                        .as_ref()
                        .and_then(|v| v["request"].as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty());
                    let task_file = start_val
                        .as_ref()
                        .and_then(|v| v["task_file"].as_str())
                        .map(|s| s.to_string());
                    let task = match task_file.as_deref() {
                        Some(p) => match read_optional_string(Path::new(p), &mut problems) {
                            OptionalFile::Ok(s) => s,
                            OptionalFile::Missing | OptionalFile::Err => String::new(),
                        },
                        None => String::new(),
                    };
                    let mut file_tags = Vec::new();
                    if let Some(tags_arr) = start_val.as_ref().and_then(|v| v["tags"].as_array()) {
                        for t in tags_arr {
                            if let Some(s) = t.as_str() {
                                file_tags.push(s.to_string());
                            }
                        }
                    }
                    if let Some(s) = start_val
                        .as_ref()
                        .and_then(|v| v["session"].as_str())
                        .filter(|s| !s.is_empty())
                    {
                        let prefix: String = s.chars().take(8).collect();
                        let tag = format!("session:{prefix}");
                        if !file_tags.contains(&tag) {
                            file_tags.push(tag);
                        }
                    }

                    let summary_path = wt_dir.join(format!("{id}.json"));
                    let mut commits = Vec::new();
                    let mut changed_files = Vec::new();
                    let mut summary_final_msg = None;
                    let mut worktree = None;
                    if let OptionalFile::Ok(val) =
                        read_optional_json::<serde_json::Value>(&summary_path, &mut problems)
                    {
                        if let Some(arr) = val["commits"].as_array() {
                            for c in arr {
                                if let (Some(sha), Some(subject)) =
                                    (c["sha"].as_str(), c["subject"].as_str())
                                {
                                    commits.push(CommitRecord {
                                        sha: sha.to_string(),
                                        subject: subject.to_string(),
                                    });
                                }
                            }
                        }
                        if let Some(arr) = val["changed_files"].as_array() {
                            for f in arr {
                                if let Some(s) = f.as_str() {
                                    changed_files.push(s.to_string());
                                }
                            }
                        }
                        summary_final_msg = val["final_message"].as_str().map(|s| s.to_string());
                        worktree = val["worktree"].as_str().map(|s| s.to_string());
                    }

                    raw_jobs.push(RawJob {
                        id: id.clone(),
                        repo: repo.clone(),
                        root: wt_dir.parent().map(|p| p.to_path_buf()),
                        task,
                        task_file,
                        pid,
                        started_at,
                        research,
                        start_plan,
                        start_plan_task,
                        request,
                        file_tags,
                        log_path,
                        report_path,
                        tags_path: wt_dir.join(format!("{id}.tags.json")),
                        verdict_path: wt_dir.join(format!("{id}.verdict.json")),
                        worktree,
                        commits,
                        changed_files,
                        summary_final_msg,
                        is_runs_dir: false,
                        problems,
                        problem_mtime: start_mtime,
                    });
                }
            }
        }
    }

    // 2. Runs-dir layout
    for runs_dir in &src.runs_dirs {
        if let Ok(repo_entries) = std::fs::read_dir(runs_dir) {
            for repo_entry in repo_entries.flatten() {
                let repo_path = repo_entry.path();
                if repo_path.is_dir() {
                    let repo = repo_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    if let Ok(entries) = std::fs::read_dir(&repo_path) {
                        for entry in entries.flatten() {
                            let fname = entry.file_name().to_string_lossy().to_string();
                            if let Some(id) = fname.strip_suffix(".log") {
                                let id = id.to_string();
                                let key = format!("{repo}/{id}");
                                if !seen_keys.insert(key) {
                                    continue;
                                }

                                let report_path = repo_path.join(format!("{id}.report.json"));
                                let log_path = repo_path.join(format!("{id}.log"));
                                let report_mtime = file_mtime(&report_path);
                                let log_mtime = file_mtime(&log_path);

                                if report_mtime.is_some_and(|m| m < cutoff)
                                    && log_mtime.is_none_or(|m| m < cutoff)
                                {
                                    continue;
                                }

                                let mut problems = Vec::new();
                                let pid_path = repo_path.join(format!("{id}.pid"));
                                let (pid, started_at) =
                                    match read_optional_string(&pid_path, &mut problems) {
                                        OptionalFile::Ok(pid_text) => {
                                            let trimmed = pid_text.trim();
                                            let pid = if trimmed.is_empty() {
                                                None
                                            } else {
                                                match trimmed.parse::<i64>() {
                                                    Ok(p) => Some(p),
                                                    Err(e) => {
                                                        problems.push(format!(
                                                            "{}: {e}",
                                                            pid_path.display()
                                                        ));
                                                        None
                                                    }
                                                }
                                            };
                                            let mtime = file_mtime(&pid_path);
                                            (pid, mtime)
                                        }
                                        OptionalFile::Missing | OptionalFile::Err => (None, None),
                                    };

                                if report_mtime.is_none() && log_mtime.is_none_or(|m| m < cutoff) {
                                    let is_alive = pid.is_some_and(|p| {
                                        p > 0
                                            && unsafe { libc::kill(p as libc::pid_t, 0) == 0 }
                                            && started_with_job(p, started_at, now)
                                    });
                                    if !is_alive {
                                        continue;
                                    }
                                }

                                let prompt_path = repo_path.join(format!("{id}.prompt"));
                                let task = match read_optional_string(&prompt_path, &mut problems) {
                                    OptionalFile::Ok(s) => s,
                                    OptionalFile::Missing | OptionalFile::Err => String::new(),
                                };

                                let worktree_path = repo_path.join(format!("{id}.worktree"));
                                let worktree =
                                    match read_optional_string(&worktree_path, &mut problems) {
                                        OptionalFile::Ok(s) => {
                                            let trimmed = s.trim().to_string();
                                            if trimmed.is_empty() {
                                                None
                                            } else {
                                                Some(trimmed)
                                            }
                                        }
                                        OptionalFile::Missing | OptionalFile::Err => None,
                                    };

                                let root = worktree.as_ref().and_then(|wt_str| {
                                    let p = Path::new(wt_str.trim_end_matches('/'));
                                    if let Some(common) = git_common_dir(p) {
                                        if common.file_name().and_then(|n| n.to_str())
                                            == Some(".git")
                                        {
                                            if let Some(parent) = common.parent() {
                                                return Some(parent.to_path_buf());
                                            }
                                        }
                                    }
                                    if p.parent()
                                        .and_then(|par| par.file_name())
                                        .and_then(|n| n.to_str())
                                        == Some(".wt")
                                    {
                                        return p
                                            .parent()
                                            .and_then(|par| par.parent())
                                            .map(Path::to_path_buf);
                                    }
                                    if let (Some(par), Some(grandpar)) =
                                        (p.parent(), p.parent().and_then(|par| par.parent()))
                                    {
                                        if par.file_name().and_then(|n| n.to_str())
                                            == Some("worktrees")
                                            && grandpar.file_name().and_then(|n| n.to_str())
                                                == Some(".claude")
                                        {
                                            return grandpar.parent().map(Path::to_path_buf);
                                        }
                                    }
                                    if let Some(file_name) = p.file_name().and_then(|n| n.to_str())
                                    {
                                        if let Some((base, _)) = file_name.split_once("--") {
                                            if !base.is_empty() {
                                                return p.parent().map(|par| par.join(base));
                                            }
                                        }
                                    }
                                    Some(p.to_path_buf())
                                });

                                raw_jobs.push(RawJob {
                                    id: id.clone(),
                                    repo: repo.clone(),
                                    root,
                                    task,
                                    task_file: None,
                                    pid,
                                    started_at,
                                    research: false,
                                    start_plan: None,
                                    start_plan_task: None,
                                    request: None,
                                    file_tags: Vec::new(),
                                    log_path,
                                    report_path,
                                    tags_path: repo_path.join(format!("{id}.tags.json")),
                                    verdict_path: repo_path.join(format!("{id}.verdict.json")),
                                    worktree,
                                    commits: Vec::new(),
                                    changed_files: Vec::new(),
                                    summary_final_msg: None,
                                    is_runs_dir: true,
                                    problems,
                                    problem_mtime: log_mtime,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let mut jobs = Vec::new();
    let mut plan_cache: std::collections::HashMap<PathBuf, Option<String>> =
        std::collections::HashMap::new();

    for raw in raw_jobs {
        let title = job_title(&raw.task, &raw.id);
        let plan = extract_plan(&raw.task);
        let prompt_task_title = extract_prompt_task_title(&raw.task);
        let plan_file = raw.start_plan.clone().or_else(|| plan.clone());
        let plan_task = raw.start_plan_task.clone().or_else(|| {
            prompt_task_title
                .clone()
                .or_else(|| extract_first_task_id(&raw.task))
        });

        let task_content = if let (Some(ref p_str), Some(ref t_title)) = (&plan, &prompt_task_title)
        {
            let p_path = if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                expand_home(p_str, &home)
            } else {
                PathBuf::from(p_str)
            };
            let cached_plan = plan_cache
                .entry(p_path.clone())
                .or_insert_with(|| std::fs::read_to_string(&p_path).ok());
            if let Some(plan_text) = cached_plan {
                extract_task_from_plan(plan_text, t_title).unwrap_or_else(|| raw.task.clone())
            } else {
                raw.task.clone()
            }
        } else {
            raw.task.clone()
        };

        let mut problems = raw.problems;

        let mut file_tags = raw.file_tags.clone();
        if let OptionalFile::Ok(arr) =
            read_optional_json::<Vec<String>>(&raw.tags_path, &mut problems)
        {
            for t in arr {
                if !file_tags.contains(&t) {
                    file_tags.push(t);
                }
            }
        }

        let mut tags = Vec::new();
        tags.push(format!("repo: {}", raw.repo));
        if let Some(ref p) = plan {
            if let Some(stem) = Path::new(p).file_stem().and_then(|s| s.to_str()) {
                tags.push(format!("plan: {stem}"));
            }
        }
        for t in &file_tags {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }

        let (verdict, verdict_notes, verdict_ts) =
            match read_optional_json::<serde_json::Value>(&raw.verdict_path, &mut problems) {
                OptionalFile::Ok(val) => {
                    let v = val["verdict"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    if v.is_none() {
                        problems.push(format!(
                            "{}: missing required field 'verdict'",
                            raw.verdict_path.display()
                        ));
                    }
                    let notes = val["notes"].as_str().map(|s| s.to_string());
                    let ts = val["ts"].as_u64().or_else(|| file_mtime(&raw.verdict_path));
                    (v, notes, ts)
                }
                OptionalFile::Missing | OptionalFile::Err => (None, None, None),
            };

        let log_info = parse_log(&raw.log_path, &mut problems);

        let report_state = read_optional_json::<serde_json::Value>(&raw.report_path, &mut problems);
        let report_mtime = file_mtime(&raw.report_path);

        let (
            status,
            gate_running,
            cost_usd,
            turns,
            gate_attempts,
            gate_failures,
            model,
            finished_at,
            elapsed_s,
            now_str,
        ) = match report_state {
            OptionalFile::Ok(report) => {
                let outcome = match report.get("outcome") {
                    Some(v) => match v.as_str() {
                        Some(s) if !s.is_empty() => s.to_string(),
                        _ => {
                            problems.push(format!(
                                "{}: invalid 'outcome' field (expected non-empty string)",
                                raw.report_path.display()
                            ));
                            "error".to_string()
                        }
                    },
                    None => {
                        problems.push(format!(
                            "{}: missing required field 'outcome'",
                            raw.report_path.display()
                        ));
                        "error".to_string()
                    }
                };
                let cost = report["cost_usd"].as_f64();
                let turns = read_required_u64(&report, "turns", &raw.report_path, &mut problems);
                let gate_attempts =
                    read_required_u64(&report, "gate_attempts", &raw.report_path, &mut problems);
                let model = report["model_id"].as_str().map(|s| s.to_string());

                let gate_failures = match report.get("gate_failures") {
                    None | Some(serde_json::Value::Null) => Vec::new(),
                    Some(serde_json::Value::Array(arr)) => {
                        let skip = arr.len().saturating_sub(2);
                        let mut failures = Vec::new();
                        for (idx, v) in arr.iter().enumerate() {
                            match v.as_str() {
                                Some(s) => {
                                    if idx >= skip {
                                        let count = s.chars().count();
                                        if count > 1500 {
                                            failures.push(s.chars().skip(count - 1500).collect());
                                        } else {
                                            failures.push(s.to_string());
                                        }
                                    }
                                }
                                None => {
                                    problems.push(format!(
                                        "{}: gate_failures[{idx}] is not a string",
                                        raw.report_path.display()
                                    ));
                                }
                            }
                        }
                        failures
                    }
                    Some(_) => {
                        problems.push(format!(
                            "{}: invalid 'gate_failures' field (expected array)",
                            raw.report_path.display()
                        ));
                        Vec::new()
                    }
                };

                let finished_at = report_mtime;
                let elapsed_s = if let Some(dur) = report["duration_seconds"].as_f64() {
                    Some(dur.round() as u64)
                } else if let (Some(fin), Some(st)) = (finished_at, raw.started_at) {
                    if fin >= st {
                        Some(fin - st)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let report_final_msg = report["final_message"].as_str();
                let now_from_msg = raw
                    .summary_final_msg
                    .as_deref()
                    .or(report_final_msg)
                    .map(extract_now_from_final_message)
                    .filter(|s| !s.is_empty());
                let now_str = if let Some(s) = now_from_msg {
                    s
                } else if let Some(ref tool) = log_info.last_tool {
                    tool.clone()
                } else {
                    String::new()
                };

                (
                    outcome,
                    false,
                    cost,
                    turns,
                    gate_attempts,
                    gate_failures,
                    model,
                    finished_at,
                    elapsed_s,
                    now_str,
                )
            }
            OptionalFile::Err => {
                let finished_at = report_mtime;
                let elapsed_s = match (finished_at, raw.started_at) {
                    (Some(fin), Some(st)) if fin >= st => Some(fin - st),
                    _ => None,
                };
                let now_str = log_info.last_tool.clone().unwrap_or_default();
                (
                    "error".to_string(),
                    false,
                    None,
                    log_info.turns,
                    log_info.gate_attempts,
                    Vec::new(),
                    None,
                    finished_at,
                    elapsed_s,
                    now_str,
                )
            }
            OptionalFile::Missing => {
                let is_alive = raw.pid.is_some_and(|p| {
                    p > 0
                        && unsafe { libc::kill(p as libc::pid_t, 0) == 0 }
                        && (!raw.is_runs_dir || started_with_job(p, raw.started_at, now))
                });
                let status = if is_alive {
                    "running".to_string()
                } else {
                    "stopped".to_string()
                };
                let gate_running = if status == "running" {
                    log_info.gate_running
                } else {
                    false
                };
                let cost = None;
                let turns = log_info.turns;
                let gate_attempts = log_info.gate_attempts;
                let gate_failures = Vec::new();
                let model = None;
                let finished_at = None;
                let elapsed_s = if status == "running" {
                    raw.started_at.map(|s| now.saturating_sub(s))
                } else {
                    None
                };
                let now_str = log_info.last_tool.clone().unwrap_or_default();

                (
                    status,
                    gate_running,
                    cost,
                    turns,
                    gate_attempts,
                    gate_failures,
                    model,
                    finished_at,
                    elapsed_s,
                    now_str,
                )
            }
        };

        let keep = if status == "running" {
            true
        } else {
            let effective_finished = finished_at.or(log_info.mtime).or(if !problems.is_empty() {
                raw.problem_mtime
            } else {
                None
            });
            if let Some(fin) = effective_finished {
                fin >= cutoff
            } else {
                false
            }
        };
        if !keep {
            continue;
        }

        let task_truncated: String = task_content.chars().take(2000).collect();
        let worktree_exists = raw.worktree.as_deref().map(|wt| Path::new(wt).is_dir());

        jobs.push(Job {
            key: format!("{}/{}", raw.repo, raw.id),
            id: raw.id,
            repo: raw.repo,
            status,
            gate_running,
            title,
            task: task_truncated,
            plan,
            now: now_str,
            started_at: raw.started_at,
            finished_at,
            elapsed_s,
            cost_usd,
            turns,
            gate_attempts,
            gate_failures,
            model,
            last_tool: log_info.last_tool,
            worktree: raw.worktree,
            worktree_exists,
            log: raw.log_path.display().to_string(),
            commits: raw.commits,
            changed_files: raw.changed_files,
            tags,
            verdict,
            problems,
            root: raw.root,
            plan_file,
            plan_task,
            request: raw.request,
            task_file: raw.task_file,
            start_plan: raw.start_plan,
            file_tags,
            verdict_notes,
            verdict_ts,
            log_mtime: log_info.mtime,
            research: raw.research,
        });
    }

    // Sort jobs: running jobs first, newest started_at first; then the rest, newest finished_at first.
    jobs.sort_by(|a, b| {
        let a_running = a.status == "running";
        let b_running = b.status == "running";
        match (a_running, b_running) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => b
                .started_at
                .unwrap_or(0)
                .cmp(&a.started_at.unwrap_or(0))
                .then_with(|| a.key.cmp(&b.key)),
            (false, false) => {
                let a_fin = a.finished_at.or(a.log_mtime).unwrap_or(0);
                let b_fin = b.finished_at.or(b.log_mtime).unwrap_or(0);
                b_fin.cmp(&a_fin).then_with(|| a.key.cmp(&b.key))
            }
        }
    });

    jobs
}

pub fn collect(src: &Sources, now: u64) -> serde_json::Value {
    let jobs = collect_jobs(src, now);
    let running = jobs.iter().filter(|j| j.status == "running").count();
    let last_24h = jobs
        .iter()
        .filter(|j| {
            j.started_at
                .is_some_and(|s| s >= now.saturating_sub(24 * 3600))
        })
        .count();
    let repos = jobs.iter().map(|j| &j.repo).collect::<HashSet<_>>().len();

    let finished_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| {
            j.status != "running"
                && j.finished_at
                    .or(j.log_mtime)
                    .is_some_and(|f| f >= now.saturating_sub(24 * 3600))
        })
        .collect();

    let finished_24h = finished_jobs.len();
    let passed_24h = finished_jobs
        .iter()
        .filter(|j| j.status == "verified")
        .count();
    let sum_cost: f64 = finished_jobs.iter().filter_map(|j| j.cost_usd).sum();
    let cost_24h_usd = (sum_cost * 100.0).round() / 100.0;

    let response = DashboardResponse {
        generated_at: now,
        summary: Summary {
            running,
            last_24h,
            repos,
            finished_24h,
            passed_24h,
            cost_24h_usd,
        },
        jobs,
    };

    serde_json::to_value(&response).unwrap()
}

pub(crate) fn shared_asset(clean_path: &str) -> Option<(u16, &'static str, Vec<u8>)> {
    match clean_path {
        "/favicon.svg" => {
            let svg = include_bytes!("../assets/dashboard/favicon.svg");
            Some((200, "image/svg+xml", svg.to_vec()))
        }
        "/fonts/archivo.woff2" => {
            let font = include_bytes!("../assets/dashboard/fonts/archivo.woff2");
            Some((200, "font/woff2", font.to_vec()))
        }
        "/fonts/plex-mono-400.woff2" => {
            let font = include_bytes!("../assets/dashboard/fonts/plex-mono-400.woff2");
            Some((200, "font/woff2", font.to_vec()))
        }
        "/fonts/plex-mono-500.woff2" => {
            let font = include_bytes!("../assets/dashboard/fonts/plex-mono-500.woff2");
            Some((200, "font/woff2", font.to_vec()))
        }
        _ => None,
    }
}

pub fn route(path: &str, src: &Sources) -> (u16, &'static str, Vec<u8>) {
    let clean_path = match path.split_once('?') {
        Some((p, _)) => p,
        None => path,
    };
    if let Some(asset) = shared_asset(clean_path) {
        return asset;
    }
    match clean_path {
        "/" | "/index.html" => {
            let html = include_str!("../assets/dashboard/index.html");
            (200, "text/html; charset=utf-8", html.as_bytes().to_vec())
        }
        "/api/jobs" => {
            let now = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let val = collect(src, now);
            let body = serde_json::to_vec(&val).unwrap_or_default();
            (200, "application/json", body)
        }
        "/api/plans" => {
            let now = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
            let running_only = query
                .split('&')
                .any(|p| p == "running=1" || p == "running=true");
            let val = crate::plans::collect_plans_filtered(src, now, running_only);
            let body = serde_json::to_vec(&val).unwrap_or_default();
            (200, "application/json", body)
        }
        _ => (404, "text/plain; charset=utf-8", b"Not Found\n".to_vec()),
    }
}

pub(crate) fn serve_http<F>(
    listener: std::net::TcpListener,
    no_store_paths: &'static [&'static str],
    handler: F,
) where
    F: Fn(&str) -> (u16, &'static str, Vec<u8>) + Send + Sync + 'static,
{
    let handler = std::sync::Arc::new(handler);
    for stream in listener.incoming().flatten() {
        let handler = std::sync::Arc::clone(&handler);
        std::thread::spawn(move || {
            handle_http_connection(stream, no_store_paths, handler.as_ref());
        });
    }
}

fn handle_http_connection<F>(mut stream: std::net::TcpStream, no_store_paths: &[&str], handler: &F)
where
    F: Fn(&str) -> (u16, &'static str, Vec<u8>),
{
    let mut reader = std::io::BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    drop(reader);

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" {
        let not_found = b"Not Found\n";
        let header = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            not_found.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(not_found);
        let _ = stream.flush();
        return;
    }

    let path = parts[1];
    let (code, ctype, body) = handler(path);
    let clean_path = match path.split_once('?') {
        Some((p, _)) => p,
        None => path,
    };
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let extra = if no_store_paths.contains(&clean_path) {
        "Cache-Control: no-store\r\n"
    } else {
        ""
    };
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

pub fn run(args: &crate::cli::DashboardArgs) -> i32 {
    let (sources, where_from) = Sources::from_args_with_where(args);
    let listener = match std::net::TcpListener::bind(format!("127.0.0.1:{}", args.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error binding 127.0.0.1:{}: {e}", args.port);
            return 1;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error getting local address: {e}");
            return 1;
        }
    };
    let port = local_addr.port();
    let url = format!("http://127.0.0.1:{port}");
    if sources.roots.is_empty() && sources.runs_dirs.is_empty() {
        eprintln!(
            "blyte dashboard: nothing to watch yet: run blyte dispatch in a repository, or list roots in ~/.config/blyte/dashboard.toml"
        );
    } else {
        eprintln!(
            "blyte dashboard: watching {} repositories and {} run directories ({where_from})",
            sources.roots.len(),
            sources.runs_dirs.len()
        );
    }
    println!("blyte dashboard: {url}");
    let _ = std::io::stdout().flush();

    if args.open {
        let cmd = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(cmd).arg(&url).spawn() {
            eprintln!("warning: failed to open browser: {e}");
        }
    }

    let live = args.roots.is_empty() && args.runs_dirs.is_empty();
    let args = args.clone();
    serve_http(listener, &["/api/jobs", "/api/plans"], move |path| {
        if live {
            let src = Sources::from_args(&args);
            route(path, &src)
        } else {
            route(path, &sources)
        }
    });

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_first_markdown_heading() {
        assert_eq!(
            extract_title(
                "From the plan docs/p.md:\n\n### Task G2: an empty gate is skipped\n\nbody"
            ),
            "Task G2: an empty gate is skipped"
        );
    }

    #[test]
    fn extract_title_no_heading_fallback() {
        assert_eq!(extract_title("fix the bug\nmore"), "fix the bug");
    }

    #[test]
    fn extract_title_task_rule_wins() {
        assert_eq!(
            extract_title("# Heading\nTask: \"explicit title\"\nmore"),
            "explicit title"
        );
    }

    #[test]
    fn job_title_strips_task_id_prefix() {
        let text1 = "From the plan docs/p.md:\n\n### Task G2: an empty gate is skipped\n\nbody";
        assert_eq!(job_title(text1, "G2"), "an empty gate is skipped");
        assert_eq!(job_title("Task G3: x", "G2"), "Task G3: x");
        assert_eq!(job_title("Task G2:", "G2"), "Task G2:");
    }

    #[test]
    fn session_tag_from_start_file() {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("repos/repo-a/.wt");
        std::fs::create_dir_all(&wt).unwrap();

        let me = std::process::id();
        std::fs::write(
            wt.join("T1.start.json"),
            format!(r#"{{"id":"T1","pid":{me},"session":"4f83bf80-a445-4090","tags":["x"]}}"#),
        )
        .unwrap();
        std::fs::write(
            wt.join("T2.start.json"),
            format!(r#"{{"id":"T2","pid":{me},"session":"abc"}}"#),
        )
        .unwrap();
        std::fs::write(
            wt.join("T3.start.json"),
            format!(r#"{{"id":"T3","pid":{me},"tags":["x"]}}"#),
        )
        .unwrap();
        std::fs::write(
            wt.join("T4.start.json"),
            format!(r#"{{"id":"T4","pid":{me},"session":"4f83bf80-a445-4090","tags":["session:4f83bf80"]}}"#),
        )
        .unwrap();

        let src = Sources {
            roots: vec![d.path().join("repos")],
            runs_dirs: vec![],
            hours: 24,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let jobs = collect_jobs(&src, now);

        let t1 = jobs.iter().find(|j| j.id == "T1").unwrap();
        assert!(t1.tags.contains(&"x".to_string()));
        assert!(t1.tags.contains(&"session:4f83bf80".to_string()));

        let t2 = jobs.iter().find(|j| j.id == "T2").unwrap();
        assert!(t2.tags.contains(&"session:abc".to_string()));

        let t3 = jobs.iter().find(|j| j.id == "T3").unwrap();
        assert!(!t3.tags.iter().any(|t| t.starts_with("session:")));

        let t4 = jobs.iter().find(|j| j.id == "T4").unwrap();
        assert_eq!(
            t4.tags
                .iter()
                .filter(|t| t.as_str() == "session:4f83bf80")
                .count(),
            1
        );
    }

    #[test]
    fn strip_ansi_borrows_plain_and_strips_csi_sequences() {
        let plain = "[Turn 4]\n● bash\n✓ Gate passed\n";
        let res = strip_ansi(plain);
        assert!(matches!(res, std::borrow::Cow::Borrowed(_)));
        assert_eq!(res.as_ref(), plain);

        let colored =
            "\x1b[32m[Turn 4]\x1b[0m\n\x1b[1m● bash\x1b[0m\n\x1b[32m✓ Gate passed\x1b[0m\n";
        let stripped = strip_ansi(colored);
        assert!(matches!(stripped, std::borrow::Cow::Owned(_)));
        assert_eq!(stripped.as_ref(), plain);

        assert_eq!(strip_ansi("\x1b[32\nhello").as_ref(), "\nhello");
        assert_eq!(strip_ansi("\x1b[32\x1b[0mhello").as_ref(), "\x1b[0mhello");
        assert_eq!(strip_ansi("\x1bXhello").as_ref(), "\x1bXhello");
        assert_eq!(strip_ansi("tail\x1b").as_ref(), "tail\x1b");
        assert_eq!(strip_ansi("\x1b[32").as_ref(), "");
        assert_eq!(strip_ansi("\x1b[✓mhello").as_ref(), "hello");
    }

    #[test]
    fn git_common_dir_resolves_relative_gitdir_and_commondir() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("myrepo");
        let worktree_git = main.join(".git/worktrees/wt1");
        let wt = tmp.path().join("wt1");
        std::fs::create_dir_all(&worktree_git).unwrap();
        std::fs::create_dir_all(wt.join("nested")).unwrap();
        std::fs::write(worktree_git.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), "gitdir: ../myrepo/.git/worktrees/wt1\n").unwrap();

        let common = git_common_dir(&wt.join("nested")).unwrap();
        assert_eq!(common.file_name().and_then(|n| n.to_str()), Some(".git"));
        assert_eq!(
            common
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some("myrepo")
        );
        assert_eq!(repo_name(&wt.join("nested")), "myrepo");
    }
}
