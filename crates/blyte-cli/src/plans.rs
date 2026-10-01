//! Plan parser and plans collector for the dashboard (plan 2026-10-01).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::dashboard::{self, Job, Sources};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanDoc {
    pub title: String,
    pub tasks: Vec<PlanTask>,
    pub decisions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub heading: String,
    pub deps: Vec<String>,
    pub lane: String,
    pub files: Vec<String>,
}

fn parse_task_deps(lines: &[&str]) -> Vec<String> {
    let dep_line = lines.iter().find(|l| {
        let plain = l.replace("**", "").replace("__", "");
        plain.to_lowercase().contains("depends on:")
    });

    let target_line = if let Some(l) = dep_line {
        *l
    } else if let Some(l) = lines.iter().find(|l| {
        let plain = l.replace("**", "").replace("__", "");
        plain.trim_start().to_lowercase().starts_with("needs task")
    }) {
        *l
    } else {
        return Vec::new();
    };

    let plain = target_line.replace("**", "").replace("__", "");
    let lower = plain.to_lowercase();

    if let Some(pos) = lower.find("depends on:") {
        let after = &lower[pos + "depends on:".len()..];
        let trimmed = after.trim().trim_matches(['.', '*', '`', '_', ';']).trim();
        if trimmed == "none" || trimmed.starts_with("none ") || trimmed.starts_with("none,") {
            return Vec::new();
        }
    } else if lower.contains("none") {
        return Vec::new();
    }

    extract_task_ids_from_line(&plain)
}

fn extract_task_ids_from_line(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i + 4 <= len {
        let slice: String = chars[i..i + 4].iter().collect();
        if slice.eq_ignore_ascii_case("task") {
            let prev_ok = i == 0 || !chars[i - 1].is_alphanumeric();
            let next_ok = i + 4 < len && (chars[i + 4] == ' ' || chars[i + 4] == '\t');
            if prev_ok && next_ok {
                let mut j = i + 4;
                while j < len && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                let id_start = j;
                while j < len && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '-')
                {
                    j += 1;
                }
                let token: String = chars[id_start..j].iter().collect();
                if !token.is_empty() {
                    let id = if token.chars().all(|c| c.is_ascii_digit()) {
                        format!("T{token}")
                    } else {
                        token
                    };
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    ids
}

fn parse_task_files(lines: &[&str]) -> Vec<String> {
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("**Files:**")
            || trimmed.starts_with("Files:")
            || trimmed.contains("**Files:**")
            || trimmed.contains("Files:")
        {
            let mut files = Vec::new();
            let mut in_backtick = false;
            let mut current = String::new();
            for c in line.chars() {
                if c == '`' {
                    if in_backtick {
                        let path = current.trim().to_string();
                        if (path.contains('/') || path.contains('.'))
                            && !path.contains(' ')
                            && !path.is_empty()
                        {
                            files.push(path);
                        }
                        current.clear();
                        in_backtick = false;
                    } else {
                        in_backtick = true;
                    }
                } else if in_backtick {
                    current.push(c);
                }
            }
            return files;
        }
    }
    Vec::new()
}

fn parse_task_lane(lines: &[&str], files: &[String]) -> String {
    for line in lines {
        let trimmed = line.trim();
        let plain = trimmed.replace("**", "");
        if let Some(rest) = plain.strip_prefix("Lane:") {
            let l = rest.trim();
            if !l.is_empty() {
                return l.to_string();
            }
        }
    }
    if let Some(first_file) = files.first() {
        if let Some(rest) = first_file.strip_prefix("crates/") {
            let x = rest.split('/').next().unwrap_or(rest);
            x.to_string()
        } else {
            let first_comp = first_file.split('/').next().unwrap_or(first_file);
            first_comp.to_string()
        }
    } else {
        "Tasks".to_string()
    }
}

pub fn parse_plan(text: &str) -> PlanDoc {
    let mut title = String::new();
    let mut in_code_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if !in_code_fence && trimmed.starts_with("# ") {
            let t = trimmed["# ".len()..].trim();
            let t = t
                .strip_suffix(" — Plan")
                .or_else(|| t.strip_suffix(" – Plan"))
                .or_else(|| t.strip_suffix(" - Plan"))
                .unwrap_or(t);
            title = t.trim().to_string();
            break;
        }
    }

    struct Heading {
        level: usize,
        text: String,
        line_idx: usize,
    }

    let lines: Vec<&str> = text.lines().collect();
    let mut headings = Vec::new();
    in_code_fence = false;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=4).contains(&hashes) {
            let after = &trimmed[hashes..];
            if after.starts_with(' ') || after.starts_with('\t') {
                headings.push(Heading {
                    level: hashes,
                    text: after.trim().to_string(),
                    line_idx: idx,
                });
            }
        }
    }

    let mut tasks = Vec::new();
    let mut decisions = Vec::new();

    for (h_idx, h) in headings.iter().enumerate() {
        let mut end_line = lines.len();
        for next_h in &headings[h_idx + 1..] {
            if next_h.level <= h.level {
                end_line = next_h.line_idx;
                break;
            }
        }
        let section_lines = &lines[h.line_idx + 1..end_line];

        if (2..=4).contains(&h.level) {
            if let Some(after_task) = h.text.strip_prefix("Task ") {
                if let Some((n_part, title_part)) = after_task.split_once(':') {
                    let n = n_part.trim();
                    let task_title = title_part.trim();
                    if !n.is_empty() && !task_title.is_empty() {
                        let id = if n.chars().all(|c| c.is_ascii_digit()) {
                            format!("T{n}")
                        } else {
                            n.to_string()
                        };
                        let heading = h.text.clone();
                        let deps = parse_task_deps(section_lines);
                        let files = parse_task_files(section_lines);
                        let lane = parse_task_lane(section_lines, &files);

                        tasks.push(PlanTask {
                            id,
                            title: task_title.to_string(),
                            heading,
                            deps,
                            lane,
                            files,
                        });
                    }
                }
            }
        }

        if h.text.eq_ignore_ascii_case("Decisions") {
            for line in section_lines {
                let trimmed = line.trim();
                if let Some(bullet) = trimmed.strip_prefix("- ") {
                    let b = bullet.trim();
                    if !b.is_empty() {
                        decisions.push(b.to_string());
                    }
                }
            }
        }
    }

    PlanDoc {
        title,
        tasks,
        decisions,
    }
}

fn matches_task(task: &PlanTask, pt: &str) -> bool {
    let pt = pt.trim();
    if pt.is_empty() {
        return false;
    }
    if pt == task.heading || pt == task.title || pt == task.id {
        return true;
    }
    if let Some(after_task) = task.heading.strip_prefix("Task ") {
        let n = after_task
            .split_once(':')
            .map(|(n, _)| n.trim())
            .unwrap_or(after_task.trim());
        let prefix = format!("Task {n}");
        if let Some(rest) = pt.strip_prefix(&prefix) {
            if rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '_') {
                return true;
            }
        }
    }
    false
}

fn is_ancestor(job: &Job, root: &Path) -> bool {
    let Some(last_commit) = job.commits.last() else {
        return false;
    };
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &last_commit.sha, "HEAD"])
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct PlanItem {
    key: String,
    has_running: bool,
    latest_activity: u64,
    val: serde_json::Value,
}

struct PlanEvent {
    at: u64,
    task: String,
    text: String,
}

pub fn collect_plans(src: &Sources, now: u64) -> serde_json::Value {
    let mut plan_src = src.clone();
    plan_src.hours = plan_src.hours.max(7 * 24);
    let jobs = dashboard::collect_jobs(&plan_src, now);

    let mut groups: HashMap<(PathBuf, PathBuf), (PathBuf, Vec<&Job>)> = HashMap::new();
    for job in &jobs {
        let (Some(ref root), Some(ref plan_file)) = (&job.root, &job.plan_file) else {
            continue;
        };
        let resolved = if Path::new(plan_file).is_relative() {
            root.join(plan_file)
        } else {
            PathBuf::from(plan_file)
        };
        let entry = groups
            .entry((root.clone(), resolved))
            .or_insert_with(|| (root.clone(), Vec::new()));
        entry.1.push(job);
    }

    let mut plan_items = Vec::new();

    for ((root, resolved_path), (_, jobs_in_group)) in groups {
        let Ok(content) = std::fs::read_to_string(&resolved_path) else {
            continue;
        };
        let doc = parse_plan(&content);
        let repo = dashboard::repo_name(&root);
        let stem = resolved_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plan");
        let key = format!("{repo}/{stem}");
        let path_str = resolved_path.to_string_lossy().to_string();

        let mut lanes = Vec::new();
        let mut seen_lanes = HashSet::new();
        for t in &doc.tasks {
            if seen_lanes.insert(t.lane.clone()) {
                let id = t.lane.to_lowercase().replace(' ', "-");
                lanes.push(serde_json::json!({
                    "id": id,
                    "label": t.lane,
                }));
            }
        }

        let mut task_jobs: HashMap<String, Vec<&Job>> = HashMap::new();
        let mut all_matched_jobs: Vec<&Job> = Vec::new();

        for task in &doc.tasks {
            let mut matched = Vec::new();
            for job in &jobs_in_group {
                if let Some(ref pt) = job.plan_task {
                    if matches_task(task, pt) {
                        matched.push(*job);
                        if !all_matched_jobs.iter().any(|j| j.id == job.id) {
                            all_matched_jobs.push(*job);
                        }
                    }
                }
            }
            task_jobs.insert(task.id.clone(), matched);
        }

        let seven_days = 7 * 24 * 3600;
        let any_dispatched = all_matched_jobs.iter().any(|j| {
            j.status == "running"
                || j.started_at
                    .is_some_and(|st| st >= now.saturating_sub(seven_days))
        });
        if !any_dispatched {
            continue;
        }

        let mut task_states: HashMap<String, String> = HashMap::new();
        let mut latest_jobs: HashMap<String, &Job> = HashMap::new();

        for task in &doc.tasks {
            if let Some(m_jobs) = task_jobs.get(&task.id) {
                if let Some(latest) = m_jobs.iter().max_by_key(|j| j.started_at.unwrap_or(0)) {
                    latest_jobs.insert(task.id.clone(), *latest);
                    let state = if latest.status == "running" {
                        "running".to_string()
                    } else if latest.verdict.as_deref() == Some("redone") {
                        "redone".to_string()
                    } else if latest.status == "verified" {
                        if is_ancestor(latest, &root) {
                            "merged".to_string()
                        } else {
                            "verified".to_string()
                        }
                    } else {
                        "failed".to_string()
                    };
                    task_states.insert(task.id.clone(), state);
                }
            }
        }

        for task in &doc.tasks {
            if !task_states.contains_key(&task.id) {
                let all_deps_merged = task
                    .deps
                    .iter()
                    .all(|dep_id| task_states.get(dep_id).map(|s| s.as_str()) == Some("merged"));
                let state = if all_deps_merged { "ready" } else { "waiting" };
                task_states.insert(task.id.clone(), state.to_string());
            }
        }

        let mut tasks_json = Vec::new();
        for task in &doc.tasks {
            let state = task_states.get(&task.id).unwrap().clone();
            let lane_id = task.lane.to_lowercase().replace(' ', "-");
            let task_val = if let Some(job) = latest_jobs.get(&task.id) {
                let files = if !job.changed_files.is_empty() {
                    job.changed_files.clone()
                } else {
                    task.files.clone()
                };
                serde_json::json!({
                    "id": task.id,
                    "title": task.title,
                    "heading": task.heading,
                    "lane": lane_id,
                    "deps": task.deps,
                    "state": state,
                    "dispatch": job.key,
                    "now": job.now,
                    "gate_running": job.gate_running,
                    "gate_attempts": job.gate_attempts,
                    "elapsed_s": job.elapsed_s,
                    "cost_usd": job.cost_usd,
                    "turns": job.turns,
                    "files": files,
                    "commits": job.commits,
                    "verdict": job.verdict,
                    "notes": job.verdict_notes,
                })
            } else {
                serde_json::json!({
                    "id": task.id,
                    "title": task.title,
                    "heading": task.heading,
                    "lane": lane_id,
                    "deps": task.deps,
                    "state": state,
                    "dispatch": serde_json::Value::Null,
                    "now": "",
                    "gate_running": false,
                    "gate_attempts": 0,
                    "elapsed_s": serde_json::Value::Null,
                    "cost_usd": serde_json::Value::Null,
                    "turns": 0,
                    "files": task.files,
                    "commits": [],
                    "verdict": serde_json::Value::Null,
                    "notes": serde_json::Value::Null,
                })
            };
            tasks_json.push(task_val);
        }

        let mut events: Vec<PlanEvent> = Vec::new();
        for task in &doc.tasks {
            if let Some(m_jobs) = task_jobs.get(&task.id) {
                for job in m_jobs {
                    if let Some(st) = job.started_at {
                        events.push(PlanEvent {
                            at: st,
                            task: task.id.clone(),
                            text: "dispatched".to_string(),
                        });
                    }
                    if job.status != "running" {
                        if let Some(fin) = job.finished_at.or(job.log_mtime) {
                            events.push(PlanEvent {
                                at: fin,
                                task: task.id.clone(),
                                text: job.status.clone(),
                            });
                        }
                    }
                    if let Some(ref v) = job.verdict {
                        if let Some(ts) = job.verdict_ts {
                            events.push(PlanEvent {
                                at: ts,
                                task: task.id.clone(),
                                text: format!("verdict {v}"),
                            });
                        }
                    }
                }
            }
        }
        events.sort_by_key(|e| std::cmp::Reverse(e.at));
        let events_json: Vec<serde_json::Value> = events
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "at": e.at,
                    "task": e.task,
                    "text": e.text,
                })
            })
            .collect();

        let mut decisions_json = Vec::new();
        for d in &doc.decisions {
            decisions_json.push(serde_json::json!({
                "text": d,
                "source": "plan",
            }));
        }
        for task in &doc.tasks {
            if let Some(job) = latest_jobs.get(&task.id) {
                if let Some(ref notes) = job.verdict_notes {
                    if !notes.trim().is_empty() {
                        decisions_json.push(serde_json::json!({
                            "text": notes,
                            "source": format!("{} verdict note", task.id),
                        }));
                    }
                }
            }
        }

        let tasks_count = doc.tasks.len();
        let merged_count = task_states
            .values()
            .filter(|s| s.as_str() == "merged")
            .count();
        let verified_count = task_states
            .values()
            .filter(|s| s.as_str() == "verified")
            .count();
        let running_count = task_states
            .values()
            .filter(|s| s.as_str() == "running")
            .count();
        let not_started_count = task_states
            .values()
            .filter(|s| s.as_str() == "ready" || s.as_str() == "waiting")
            .count();
        let cost_usd: f64 = all_matched_jobs.iter().filter_map(|j| j.cost_usd).sum();
        let cost_usd = (cost_usd * 100.0).round() / 100.0;
        let first_dispatch_at = all_matched_jobs.iter().filter_map(|j| j.started_at).min();

        let intervals: Vec<(u64, u64)> = all_matched_jobs
            .iter()
            .filter_map(|j| {
                let st = j.started_at?;
                let fin = j.finished_at.unwrap_or(now);
                Some((st, fin))
            })
            .collect();
        let mut max_parallel = 0;
        for (t, _) in &intervals {
            let count = intervals
                .iter()
                .filter(|(st, fin)| *st <= *t && *t <= *fin)
                .count();
            if count > max_parallel {
                max_parallel = count;
            }
        }

        let stats_json = serde_json::json!({
            "tasks": tasks_count,
            "merged": merged_count,
            "verified": verified_count,
            "running": running_count,
            "not_started": not_started_count,
            "cost_usd": cost_usd,
            "first_dispatch_at": first_dispatch_at,
            "max_parallel": max_parallel,
        });

        let has_running = running_count > 0;
        let latest_activity = events_json
            .first()
            .and_then(|e| e["at"].as_u64())
            .or(first_dispatch_at)
            .unwrap_or(0);

        let plan_val = serde_json::json!({
            "key": key,
            "repo": repo,
            "path": path_str,
            "title": doc.title,
            "lanes": lanes,
            "tasks": tasks_json,
            "events": events_json,
            "decisions": decisions_json,
            "stats": stats_json,
        });

        plan_items.push(PlanItem {
            key,
            has_running,
            latest_activity,
            val: plan_val,
        });
    }

    plan_items.sort_by(|a, b| match (a.has_running, b.has_running) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => b
            .latest_activity
            .cmp(&a.latest_activity)
            .then_with(|| a.key.cmp(&b.key)),
    });

    serde_json::json!({
        "generated_at": now,
        "plans": plan_items.into_iter().map(|p| p.val).collect::<Vec<_>>(),
    })
}
