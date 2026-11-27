use std::io::ErrorKind;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::dashboard::{collect_jobs, repo_name, Sources};
use crate::replay::{classify_jobs, group_of, Run, Skipped};

#[derive(Clone, Debug, serde::Serialize)]
pub struct LiveRuns {
    pub generated_at: u64,
    pub repo: String,
    pub running: Vec<RunningRun>,
    pub runs: Vec<Run>,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RunningRun {
    pub key: String,
    pub id: String,
    pub research: bool,
    pub title: String,
    pub task: String,
    pub plan_file: Option<String>,
    pub plan_task: Option<String>,
    pub request: Option<String>,
    pub started_at: Option<u64>,
    pub turns: u64,
    pub gate_running: bool,
    pub activity: String,
}

pub fn collect(root: &Path, now: u64, days: u64) -> Result<LiveRuns, String> {
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }

    let repo = repo_name(root);
    let wt_dir = root.join(".wt");
    if !wt_dir.is_dir() {
        return Ok(LiveRuns {
            generated_at: now,
            repo,
            running: Vec::new(),
            runs: Vec::new(),
            skipped: Vec::new(),
        });
    }

    let expected_root = wt_dir.parent();
    let src = Sources {
        roots: vec![root.to_path_buf()],
        runs_dirs: Vec::new(),
        hours: days.saturating_mul(24),
    };
    let jobs = collect_jobs(&src, now)
        .into_iter()
        .filter(|job| job.root.as_deref() == expected_root)
        .collect();

    let (running_jobs, mut runs, mut skipped) = classify_jobs(jobs, now, days);

    let mut running: Vec<RunningRun> = running_jobs
        .into_iter()
        .map(|job| RunningRun {
            key: job.key,
            id: job.id,
            research: job.research,
            title: job.title,
            task: job.task,
            plan_file: job.plan_file,
            plan_task: job.plan_task,
            request: job.request,
            started_at: job.started_at,
            turns: job.turns,
            gate_running: job.gate_running,
            activity: job.now,
        })
        .collect();

    running.sort_by(|a, b| {
        a.started_at
            .unwrap_or(u64::MAX)
            .cmp(&b.started_at.unwrap_or(u64::MAX))
            .then_with(|| a.key.cmp(&b.key))
    });
    runs.sort_by(|a, b| {
        b.finished_at
            .cmp(&a.finished_at)
            .then_with(|| a.key.cmp(&b.key))
    });
    skipped.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(LiveRuns {
        generated_at: now,
        repo,
        running,
        runs,
        skipped,
    })
}

fn file_mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn non_empty_str(val: Option<&serde_json::Value>) -> Option<String> {
    val.and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn title_from_task(task: &str, id: &str) -> String {
    for line in task.lines() {
        let trimmed = line.trim().trim_start_matches('#').trim();
        if !trimmed.is_empty() {
            return trimmed.chars().take(120).collect();
        }
    }
    id.to_string()
}

fn parse_running_log(path: &Path) -> Result<(u64, bool, String), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((0, false, String::new())),
        Err(e) => return Err(format!("log unreadable ({}): {e}", path.display())),
    };
    let mut turns: u64 = 0;
    let mut gate_running = false;
    let mut activity = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("== turn ") || trimmed.starts_with("turn ") {
            turns = turns.saturating_add(1);
        }
        if trimmed.starts_with("== gate") {
            gate_running = true;
        } else if trimmed.starts_with("== ") {
            gate_running = false;
        }
        activity = trimmed.chars().take(120).collect();
    }
    Ok((turns, gate_running, activity))
}

fn parse_summary_seconds(val: &serde_json::Value) -> Result<u64, String> {
    let Some(sec_val) = val.get("seconds") else {
        return Err("missing seconds".to_string());
    };
    if let Some(n) = sec_val.as_u64() {
        return Ok(n);
    }
    if let Some(f) = sec_val.as_f64() {
        if f.is_finite() && f >= 0.0 && f < (u64::MAX as f64) {
            return Ok(f.floor() as u64);
        }
        return Err("seconds must be a non-negative finite number".to_string());
    }
    Err("seconds is not a number".to_string())
}

pub fn collect_in_place(
    root: &Path,
    records_dir: &Path,
    now: u64,
    days: u64,
) -> Result<LiveRuns, String> {
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }

    let repo = repo_name(root);
    if !records_dir.is_dir() {
        return Ok(LiveRuns {
            generated_at: now,
            repo,
            running: Vec::new(),
            runs: Vec::new(),
            skipped: Vec::new(),
        });
    }

    let cutoff = now.saturating_sub(days.saturating_mul(86_400));
    let mut start_entries: Vec<(String, std::path::PathBuf)> = Vec::new();
    let rd = std::fs::read_dir(records_dir)
        .map_err(|e| format!("cannot read {}: {e}", records_dir.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", records_dir.display()))?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = fname.strip_suffix(".start.json") {
            if !id.is_empty() {
                start_entries.push((id.to_string(), entry.path()));
            }
        }
    }
    start_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut running = Vec::new();
    let mut runs = Vec::new();
    let mut skipped = Vec::new();

    for (id, start_path) in start_entries {
        let key = format!("{repo}/{id}");
        let start_raw = match std::fs::read_to_string(&start_path) {
            Ok(s) => s,
            Err(e) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("start file unreadable: {e}"),
                });
                continue;
            }
        };
        let start_val: serde_json::Value = match serde_json::from_str(&start_raw) {
            Ok(v) => v,
            Err(e) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("start file unreadable: {e}"),
                });
                continue;
            }
        };

        let started_at = start_val
            .get("started_at")
            .and_then(|v| v.as_u64())
            .or_else(|| file_mtime(&start_path));
        if started_at.is_some_and(|s| s < cutoff) {
            continue;
        }

        let research = match crate::in_place_review::parse_research_flag(&start_val, &start_path) {
            Ok(b) => b,
            Err(err) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("start file unreadable: {err}"),
                });
                continue;
            }
        };
        let plan_file = non_empty_str(start_val.get("plan"));
        let plan_task = non_empty_str(start_val.get("plan_task"));
        let request = non_empty_str(start_val.get("request"));

        let task_path = match start_val.get("task_file") {
            None | Some(serde_json::Value::Null) => records_dir.join(format!("{id}.task.md")),
            Some(serde_json::Value::String(s)) if s.is_empty() => {
                records_dir.join(format!("{id}.task.md"))
            }
            Some(serde_json::Value::String(s)) => std::path::PathBuf::from(s),
            Some(_) => {
                skipped.push(Skipped {
                    key,
                    reason: "start file unreadable: task_file is not a string".to_string(),
                });
                continue;
            }
        };
        let task = match std::fs::read_to_string(&task_path) {
            Ok(t) => t,
            Err(e) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("task file unreadable ({}): {e}", task_path.display()),
                });
                continue;
            }
        };
        let title = title_from_task(&task, &id);

        let summary_path = records_dir.join(format!("{id}.json"));
        if !summary_path.exists() {
            let pid = start_val.get("pid").and_then(|v| v.as_i64());
            let is_alive = pid.is_some_and(crate::clean::is_pid_alive);
            if is_alive {
                let (turns, gate_running, activity) =
                    match parse_running_log(&records_dir.join(format!("{id}.log"))) {
                        Ok(parsed) => parsed,
                        Err(reason) => {
                            skipped.push(Skipped { key, reason });
                            continue;
                        }
                    };
                running.push(RunningRun {
                    key,
                    id,
                    research,
                    title,
                    task,
                    plan_file,
                    plan_task,
                    request,
                    started_at,
                    turns,
                    gate_running,
                    activity,
                });
            } else {
                skipped.push(Skipped {
                    key,
                    reason: "no summary file".to_string(),
                });
            }
            continue;
        }

        let Some(started_at) = started_at else {
            skipped.push(Skipped {
                key,
                reason: "no start time".to_string(),
            });
            continue;
        };

        let summary_raw = match std::fs::read_to_string(&summary_path) {
            Ok(s) => s,
            Err(e) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("summary unreadable: {e}"),
                });
                continue;
            }
        };
        let summary_val: serde_json::Value = match serde_json::from_str(&summary_raw) {
            Ok(v) => v,
            Err(e) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("summary unreadable: {e}"),
                });
                continue;
            }
        };
        let outcome = match summary_val.get("outcome").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            Some(_) => {
                skipped.push(Skipped {
                    key,
                    reason: "summary unreadable: empty outcome".to_string(),
                });
                continue;
            }
            None => {
                skipped.push(Skipped {
                    key,
                    reason: "summary unreadable: missing outcome".to_string(),
                });
                continue;
            }
        };
        let seconds = match parse_summary_seconds(&summary_val) {
            Ok(s) => s,
            Err(err) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("summary unreadable: {err}"),
                });
                continue;
            }
        };
        let Some(finished_at) = started_at.checked_add(seconds) else {
            skipped.push(Skipped {
                key,
                reason: "summary unreadable: finished_at overflow".to_string(),
            });
            continue;
        };
        let final_message = match summary_val.get("final_message") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                skipped.push(Skipped {
                    key,
                    reason: "summary unreadable: final_message is not a string".to_string(),
                });
                continue;
            }
        };
        let turns = match summary_val.get("turns") {
            Some(v) => match v.as_u64() {
                Some(n) => n,
                None => {
                    skipped.push(Skipped {
                        key,
                        reason: "summary unreadable: turns is not a non-negative integer"
                            .to_string(),
                    });
                    continue;
                }
            },
            None => {
                skipped.push(Skipped {
                    key,
                    reason: "summary unreadable: missing turns".to_string(),
                });
                continue;
            }
        };
        let cost_usd = summary_val.get("cost_usd").and_then(|v| v.as_f64());
        let mut changed_files = Vec::new();
        if let Some(arr) = summary_val.get("changed_files").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    changed_files.push(s.to_string());
                }
            }
        }

        let (checks_recorded, flags) = match crate::in_place_review::checks_for_run(
            root,
            records_dir,
            &id,
            &summary_val,
            &summary_path,
        ) {
            Ok(c) => (true, c.flags),
            Err(err) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("checks unreadable: {err}"),
                });
                continue;
            }
        };

        let verdict_path = records_dir.join(format!("{id}.verdict.json"));
        let (verdict, verdict_notes, verdict_ts) = match std::fs::read_to_string(&verdict_path) {
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(val) => {
                    let v = match val.get("verdict").and_then(|x| x.as_str()) {
                        Some(s) if !s.is_empty() => Some(s.to_string()),
                        _ => {
                            skipped.push(Skipped {
                                key,
                                reason: format!(
                                    "verdict unreadable ({}): missing or invalid verdict",
                                    verdict_path.display()
                                ),
                            });
                            continue;
                        }
                    };
                    let notes = match val.get("notes") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(serde_json::Value::String(s)) => Some(s.clone()),
                        Some(_) => {
                            skipped.push(Skipped {
                                key,
                                reason: format!(
                                    "verdict unreadable ({}): notes is not a string",
                                    verdict_path.display()
                                ),
                            });
                            continue;
                        }
                    };
                    let ts = match val.get("ts") {
                        Some(x) => match x.as_u64() {
                            Some(n) => Some(n),
                            None => {
                                skipped.push(Skipped {
                                    key,
                                    reason: format!(
                                        "verdict unreadable ({}): ts is not a non-negative integer",
                                        verdict_path.display()
                                    ),
                                });
                                continue;
                            }
                        },
                        None => {
                            skipped.push(Skipped {
                                key,
                                reason: format!(
                                    "verdict unreadable ({}): missing ts",
                                    verdict_path.display()
                                ),
                            });
                            continue;
                        }
                    };
                    (v, notes, ts)
                }
                Err(e) => {
                    skipped.push(Skipped {
                        key,
                        reason: format!("verdict unreadable ({}): {e}", verdict_path.display()),
                    });
                    continue;
                }
            },
            Err(e) if e.kind() == ErrorKind::NotFound => (None, None, None),
            Err(e) => {
                skipped.push(Skipped {
                    key,
                    reason: format!("verdict unreadable ({}): {e}", verdict_path.display()),
                });
                continue;
            }
        };

        let report = if research {
            let report_path = records_dir.join(format!("{id}.report.md"));
            match std::fs::read_to_string(&report_path) {
                Ok(content) => Some(content),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                Err(e) => {
                    skipped.push(Skipped {
                        key,
                        reason: format!("report unreadable: {e}"),
                    });
                    continue;
                }
            }
        } else {
            None
        };

        let group = group_of(&changed_files);
        let card = if research {
            "research".to_string()
        } else if matches!(outcome.as_str(), "verified" | "finished")
            && checks_recorded
            && flags.is_empty()
        {
            "clean".to_string()
        } else {
            "exception".to_string()
        };

        runs.push(Run {
            key,
            id,
            repo: repo.clone(),
            research,
            title,
            task,
            plan_file,
            plan_task,
            request,
            started_at,
            finished_at,
            outcome,
            final_message,
            turns,
            cost_usd,
            commits: Vec::new(),
            changed_files,
            group,
            checks_recorded,
            flags,
            card,
            verdict,
            verdict_notes,
            verdict_ts,
            report,
        });
    }

    running.sort_by(|a, b| {
        a.started_at
            .unwrap_or(u64::MAX)
            .cmp(&b.started_at.unwrap_or(u64::MAX))
            .then_with(|| a.key.cmp(&b.key))
    });
    runs.sort_by(|a, b| {
        b.finished_at
            .cmp(&a.finished_at)
            .then_with(|| a.key.cmp(&b.key))
    });
    skipped.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(LiveRuns {
        generated_at: now,
        repo,
        running,
        runs,
        skipped,
    })
}
