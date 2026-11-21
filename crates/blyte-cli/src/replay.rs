use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use serde::Serialize;

use crate::dashboard::{collect_jobs, CommitRecord, Sources};

#[derive(Debug, Serialize, PartialEq)]
pub struct Replay {
    pub exported_at: u64,
    pub window_days: u64,
    pub skipped: Vec<Skipped>,
    pub runs: Vec<Run>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Skipped {
    pub key: String,
    pub reason: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Run {
    pub key: String,
    pub id: String,
    pub repo: String,
    pub research: bool,
    pub title: String,
    pub task: String,
    pub plan_file: Option<String>,
    pub plan_task: Option<String>,
    pub started_at: u64,
    pub finished_at: u64,
    pub outcome: String,
    pub final_message: Option<String>,
    pub turns: u64,
    pub cost_usd: Option<f64>,
    pub commits: Vec<CommitRecord>,
    pub changed_files: Vec<String>,
    pub group: Option<String>,
    pub checks_recorded: bool,
    pub flags: Vec<String>,
    pub card: String,
    pub verdict: Option<String>,
    pub verdict_notes: Option<String>,
    pub verdict_ts: Option<u64>,
    pub report: Option<String>,
}

fn file_group(file: &str) -> String {
    let parts: Vec<&str> = file.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 && parts[0] == "crates" {
        format!("crates/{}", parts[1])
    } else if parts.len() >= 2 {
        parts[0].to_string()
    } else {
        ".".to_string()
    }
}

pub fn group_of(changed_files: &[String]) -> Option<String> {
    if changed_files.is_empty() {
        return None;
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in changed_files {
        *counts.entry(file_group(file)).or_insert(0) += 1;
    }
    let mut best: Option<(String, usize)> = None;
    for (group, count) in counts {
        let replace = match &best {
            None => true,
            Some((_, best_count)) => count > *best_count,
        };
        if replace {
            best = Some((group, count));
        }
    }
    best.map(|(group, _)| group)
}

pub fn card_of(outcome: &str, checks_recorded: bool, flags: &[String]) -> &'static str {
    if outcome == "verified" && checks_recorded && flags.is_empty() {
        "clean"
    } else {
        "exception"
    }
}

pub fn card_for_run(
    research: bool,
    outcome: &str,
    checks_recorded: bool,
    flags: &[String],
) -> &'static str {
    if research {
        "research"
    } else {
        card_of(outcome, checks_recorded, flags)
    }
}

struct DispatchSummaryFields {
    outcome: String,
    seconds: u64,
    final_message: Option<String>,
}

enum SummaryReadError {
    Missing,
    Unreadable(String),
}

fn parse_seconds(val: &serde_json::Value) -> Result<u64, String> {
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

fn read_summary(path: &Path) -> Result<DispatchSummaryFields, SummaryReadError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(SummaryReadError::Missing),
        Err(e) => return Err(SummaryReadError::Unreadable(e.to_string())),
    };
    let val: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| SummaryReadError::Unreadable(e.to_string()))?;
    let outcome = match val.get("outcome").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        Some(_) => return Err(SummaryReadError::Unreadable("empty outcome".to_string())),
        None => return Err(SummaryReadError::Unreadable("missing outcome".to_string())),
    };
    let seconds = parse_seconds(&val).map_err(SummaryReadError::Unreadable)?;
    let final_message = match val.get("final_message") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(SummaryReadError::Unreadable(
                "final_message is not a string".to_string(),
            ));
        }
    };
    Ok(DispatchSummaryFields {
        outcome,
        seconds,
        final_message,
    })
}

fn read_checks(path: &Path) -> Result<(bool, Vec<String>), String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((false, Vec::new())),
        Err(e) => return Err(e.to_string()),
    };
    let val: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let Some(arr) = val.get("flags").and_then(|v| v.as_array()) else {
        return Err("missing flags array".to_string());
    };
    let mut flags = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(s) = item.as_str() else {
            return Err("flags entry is not a string".to_string());
        };
        flags.push(s.to_string());
    }
    Ok((true, flags))
}

fn read_report(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn collect(src: &Sources, now: u64, days: u64) -> Replay {
    let mut windowed = src.clone();
    windowed.hours = days.saturating_mul(24);
    let jobs = collect_jobs(&windowed, now);
    let cutoff = now.saturating_sub(days.saturating_mul(86_400));

    let mut runs = Vec::new();
    let mut skipped = Vec::new();

    for job in jobs {
        if job.started_at.is_some_and(|s| s < cutoff) {
            continue;
        }

        if job.status == "running" {
            skipped.push(Skipped {
                key: job.key,
                reason: "still running".to_string(),
            });
            continue;
        }

        let Some(root) = job.root else {
            skipped.push(Skipped {
                key: job.key,
                reason: "no root".to_string(),
            });
            continue;
        };

        let Some(started_at) = job.started_at else {
            skipped.push(Skipped {
                key: job.key,
                reason: "no start time".to_string(),
            });
            continue;
        };

        let summary_path = root.join(".wt").join(format!("{}.json", job.id));
        let summary = match read_summary(&summary_path) {
            Ok(s) => s,
            Err(SummaryReadError::Missing) => {
                skipped.push(Skipped {
                    key: job.key,
                    reason: "no summary file".to_string(),
                });
                continue;
            }
            Err(SummaryReadError::Unreadable(err)) => {
                skipped.push(Skipped {
                    key: job.key,
                    reason: format!("summary unreadable: {err}"),
                });
                continue;
            }
        };

        let Some(finished_at) = started_at.checked_add(summary.seconds) else {
            skipped.push(Skipped {
                key: job.key,
                reason: "summary unreadable: finished_at overflow".to_string(),
            });
            continue;
        };

        let checks_path = root.join(".wt").join(format!("{}.checks.json", job.id));
        let (checks_recorded, flags) = match read_checks(&checks_path) {
            Ok(c) => c,
            Err(err) => {
                skipped.push(Skipped {
                    key: job.key,
                    reason: format!("checks unreadable: {err}"),
                });
                continue;
            }
        };

        let report = if job.research {
            let report_path = root.join(".wt").join(format!("{}.report.md", job.id));
            match read_report(&report_path) {
                Ok(r) => r,
                Err(err) => {
                    skipped.push(Skipped {
                        key: job.key,
                        reason: format!("report unreadable: {err}"),
                    });
                    continue;
                }
            }
        } else {
            None
        };

        let group = group_of(&job.changed_files);
        let card =
            card_for_run(job.research, &summary.outcome, checks_recorded, &flags).to_string();

        runs.push(Run {
            key: job.key,
            id: job.id,
            repo: job.repo,
            research: job.research,
            title: job.title,
            task: job.task,
            plan_file: job.plan_file,
            plan_task: job.plan_task,
            started_at,
            finished_at,
            outcome: summary.outcome,
            final_message: summary.final_message,
            turns: job.turns,
            cost_usd: job.cost_usd,
            commits: job.commits,
            changed_files: job.changed_files,
            group,
            checks_recorded,
            flags,
            card,
            verdict: job.verdict,
            verdict_notes: job.verdict_notes,
            verdict_ts: job.verdict_ts,
            report,
        });
    }

    runs.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.key.cmp(&b.key))
    });
    skipped.sort_by(|a, b| a.key.cmp(&b.key));

    Replay {
        exported_at: now,
        window_days: days,
        skipped,
        runs,
    }
}
