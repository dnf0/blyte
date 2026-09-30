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
        let mut roots = args.roots.clone();
        let mut runs_dirs = args.runs_dirs.clone();

        if roots.is_empty() && runs_dirs.is_empty() {
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                let config_path = home.join(".config/blyte/dashboard.toml");
                if let Ok(content) = std::fs::read_to_string(&config_path) {
                    if let Ok(cfg) = toml::from_str::<DashboardConfig>(&content) {
                        if let Some(r) = cfg.roots {
                            roots = r.into_iter().map(|p| expand_home(&p, &home)).collect();
                        }
                        if let Some(d) = cfg.runs_dirs {
                            runs_dirs = d.into_iter().map(|p| expand_home(&p, &home)).collect();
                        }
                    }
                }
            }
        }

        if roots.is_empty() && runs_dirs.is_empty() {
            let git_root = std::process::Command::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
            if let Some(root) = git_root {
                roots.push(root);
            } else {
                roots.push(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            }
        }

        Self {
            roots,
            runs_dirs,
            hours: args.hours,
        }
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
    pub log: String,
    pub commits: Vec<CommitRecord>,
    pub changed_files: Vec<String>,
    pub tags: Vec<String>,
    pub verdict: Option<String>,
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
        let trimmed = line.trim_start_matches(['#', ' ']).trim_end();
        if !trimmed.is_empty() {
            return trimmed.chars().take(120).collect();
        }
    }
    String::new()
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

struct LogInfo {
    turns: u64,
    last_tool: Option<String>,
    gate_attempts: u64,
    gate_running: bool,
    mtime: Option<u64>,
}

fn parse_log(path: &Path) -> LogInfo {
    let mtime = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let raw = std::fs::read_to_string(path).unwrap_or_default();
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

struct RawJob {
    id: String,
    repo: String,
    task: String,
    pid: Option<i64>,
    started_at: Option<u64>,
    file_tags: Vec<String>,
    log_path: PathBuf,
    report_path: PathBuf,
    tags_path: PathBuf,
    verdict_path: PathBuf,
    worktree: Option<String>,
    commits: Vec<CommitRecord>,
    changed_files: Vec<String>,
    summary_final_msg: Option<String>,
}

pub fn collect(src: &Sources, now: u64) -> serde_json::Value {
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
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        if let Ok(entries) = std::fs::read_dir(wt_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if let Some(id) = fname.strip_suffix(".start.json") {
                    let id = id.to_string();
                    let key = format!("{repo}/{id}");
                    if !seen_keys.insert(key) {
                        continue;
                    }

                    let start_path = wt_dir.join(format!("{id}.start.json"));
                    let start_val = std::fs::read_to_string(&start_path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
                    let pid = start_val.as_ref().and_then(|v| v["pid"].as_i64());
                    let started_at = start_val.as_ref().and_then(|v| v["started_at"].as_u64());
                    let task_file = start_val.as_ref().and_then(|v| v["task_file"].as_str());
                    let task = task_file
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default();
                    let mut file_tags = Vec::new();
                    if let Some(tags_arr) = start_val.as_ref().and_then(|v| v["tags"].as_array()) {
                        for t in tags_arr {
                            if let Some(s) = t.as_str() {
                                file_tags.push(s.to_string());
                            }
                        }
                    }

                    let summary_path = wt_dir.join(format!("{id}.json"));
                    let mut commits = Vec::new();
                    let mut changed_files = Vec::new();
                    let mut summary_final_msg = None;
                    let mut worktree = None;
                    if summary_path.is_file() {
                        if let Ok(content) = std::fs::read_to_string(&summary_path) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
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
                                summary_final_msg =
                                    val["final_message"].as_str().map(|s| s.to_string());
                                worktree = val["worktree"].as_str().map(|s| s.to_string());
                            }
                        }
                    }

                    raw_jobs.push(RawJob {
                        id: id.clone(),
                        repo: repo.clone(),
                        task,
                        pid,
                        started_at,
                        file_tags,
                        log_path: wt_dir.join(format!("{id}.log")),
                        report_path: wt_dir.join(format!("{id}.report.json")),
                        tags_path: wt_dir.join(format!("{id}.tags.json")),
                        verdict_path: wt_dir.join(format!("{id}.verdict.json")),
                        worktree,
                        commits,
                        changed_files,
                        summary_final_msg,
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

                                let pid_path = repo_path.join(format!("{id}.pid"));
                                let (pid, started_at) = if pid_path.is_file() {
                                    let pid_text =
                                        std::fs::read_to_string(&pid_path).unwrap_or_default();
                                    let pid = pid_text.trim().parse::<i64>().ok();
                                    let mtime = file_mtime(&pid_path);
                                    (pid, mtime)
                                } else {
                                    (None, None)
                                };

                                let prompt_path = repo_path.join(format!("{id}.prompt"));
                                let task = if prompt_path.is_file() {
                                    std::fs::read_to_string(&prompt_path).unwrap_or_default()
                                } else {
                                    String::new()
                                };

                                let worktree_path = repo_path.join(format!("{id}.worktree"));
                                let worktree = if worktree_path.is_file() {
                                    std::fs::read_to_string(&worktree_path)
                                        .ok()
                                        .map(|s| s.trim().to_string())
                                        .filter(|s| !s.is_empty())
                                } else {
                                    None
                                };

                                raw_jobs.push(RawJob {
                                    id: id.clone(),
                                    repo: repo.clone(),
                                    task,
                                    pid,
                                    started_at,
                                    file_tags: Vec::new(),
                                    log_path: repo_path.join(format!("{id}.log")),
                                    report_path: repo_path.join(format!("{id}.report.json")),
                                    tags_path: repo_path.join(format!("{id}.tags.json")),
                                    verdict_path: repo_path.join(format!("{id}.verdict.json")),
                                    worktree,
                                    commits: Vec::new(),
                                    changed_files: Vec::new(),
                                    summary_final_msg: None,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let mut jobs = Vec::new();

    for raw in raw_jobs {
        let title = extract_title(&raw.task);
        let plan = extract_plan(&raw.task);

        let mut tags = Vec::new();
        tags.push(format!("repo: {}", raw.repo));
        if let Some(ref p) = plan {
            if let Some(stem) = Path::new(p).file_stem().and_then(|s| s.to_str()) {
                tags.push(format!("plan: {stem}"));
            }
        }
        for t in raw.file_tags {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        if raw.tags_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&raw.tags_path) {
                if let Ok(arr) = serde_json::from_str::<Vec<String>>(&content) {
                    for t in arr {
                        if !tags.contains(&t) {
                            tags.push(t);
                        }
                    }
                }
            }
        }

        let verdict = if raw.verdict_path.is_file() {
            std::fs::read_to_string(&raw.verdict_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["verdict"].as_str().map(|s| s.to_string()))
        } else {
            None
        };

        let log_info = parse_log(&raw.log_path);

        let report_json = if raw.report_path.is_file() {
            std::fs::read_to_string(&raw.report_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        } else {
            None
        };

        let report_mtime = if raw.report_path.is_file() {
            file_mtime(&raw.report_path)
        } else {
            None
        };

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
        ) = if let Some(ref report) = report_json {
            let outcome = report["outcome"].as_str().unwrap_or("unknown").to_string();
            let cost = report["cost_usd"].as_f64();
            let turns = report["turns"].as_u64().unwrap_or(log_info.turns);
            let gate_attempts = report["gate_attempts"]
                .as_u64()
                .unwrap_or(log_info.gate_attempts);
            let model = report["model_id"].as_str().map(|s| s.to_string());

            let gate_failures = report["gate_failures"]
                .as_array()
                .map(|arr| {
                    let skip = arr.len().saturating_sub(2);
                    arr.iter()
                        .skip(skip)
                        .map(|v| {
                            let s = v.as_str().unwrap_or("");
                            let count = s.chars().count();
                            if count > 1500 {
                                s.chars().skip(count - 1500).collect()
                            } else {
                                s.to_string()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

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
            let now_str = if let Some(msg) = raw.summary_final_msg.as_deref().or(report_final_msg) {
                let first_line = msg.lines().next().unwrap_or("");
                first_line.chars().take(200).collect()
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
        } else {
            let is_alive = raw
                .pid
                .is_some_and(|p| p > 0 && unsafe { libc::kill(p as libc::pid_t, 0) == 0 });
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
        };

        // Rule 7: Kept: every running job, and any other job whose finished_at (or, with no
        // report, the log's modified time) is within hours of now.
        let keep = if status == "running" {
            true
        } else {
            let effective_finished = finished_at.or(log_info.mtime);
            if let Some(fin) = effective_finished {
                fin >= now.saturating_sub(src.hours * 3600)
            } else {
                false
            }
        };
        if !keep {
            continue;
        }

        let task_truncated: String = raw.task.chars().take(2000).collect();

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
            log: raw.log_path.display().to_string(),
            commits: raw.commits,
            changed_files: raw.changed_files,
            tags,
            verdict,
            log_mtime: log_info.mtime,
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

pub fn route(path: &str, src: &Sources) -> (u16, &'static str, Vec<u8>) {
    let clean_path = path.split('?').next().unwrap_or(path);
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
        _ => (404, "text/plain; charset=utf-8", b"Not Found\n".to_vec()),
    }
}

fn handle_connection(mut stream: std::net::TcpStream, src: &Sources) {
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
    let (code, ctype, body) = route(path, src);
    let clean_path = path.split('?').next().unwrap_or(path);
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        _ => "OK",
    };
    let extra = if clean_path == "/api/jobs" {
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
    let sources = Sources::from_args(args);
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

    let src = std::sync::Arc::new(sources);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let src = std::sync::Arc::clone(&src);
                std::thread::spawn(move || {
                    handle_connection(stream, &src);
                });
            }
            Err(_) => {
                // Ignore transient errors
            }
        }
    }

    0
}
