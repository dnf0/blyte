use std::path::Path;

use crate::dashboard::{collect_jobs, repo_name, Sources};
use crate::replay::{classify_jobs, Run, Skipped};

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
