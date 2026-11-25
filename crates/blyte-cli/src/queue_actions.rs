use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::ReviewVerdict;
use crate::dispatch::{host_commands, outside_reads, refused_reads};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hold {
    pub note: String,
    pub ts: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Accept,
    SendBack,
    Redo,
    Hold,
    MarkRead,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Decision {
    pub run: String,
    pub action: Action,
    pub note: Option<String>,
    pub conversation_message: Option<String>,
}

pub(crate) struct ReviewComputation {
    pub(crate) wt: PathBuf,
    pub(crate) wt_exists: bool,
    pub(crate) is_research: bool,
    pub(crate) base: String,
    pub(crate) task_text: Option<String>,
    pub(crate) changed_files: Vec<String>,
    pub(crate) commits_out: String,
    pub(crate) files_out: String,
    pub(crate) tree_out: String,
    pub(crate) diff_out: String,
    pub(crate) hcmds: Vec<String>,
    pub(crate) reads: Vec<String>,
    pub(crate) report_val: Option<serde_json::Value>,
    pub(crate) quote_problems: Option<Vec<String>>,
    pub(crate) claim_check: Option<blyte_agent::claim_check::ClaimCheck>,
    pub(crate) claim_check_skipped: Option<String>,
    pub(crate) checks: crate::review_checks::Checks,
    pub(crate) fallbacks: Vec<crate::fallback_check::Finding>,
}

pub fn record_verdict(
    root: &Path,
    id: &str,
    verdict: ReviewVerdict,
    notes: &str,
    now: u64,
) -> Result<(), String> {
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir)
        .map_err(|e| format!("error creating directory {}: {e}", wt_dir.display()))?;
    let verdict_path = wt_dir.join(format!("{id}.verdict.json"));
    let verdict_obj = serde_json::json!({
        "verdict": verdict.as_str(),
        "notes": notes,
        "ts": now,
    });
    let json_str = serde_json::to_string_pretty(&verdict_obj)
        .map_err(|e| format!("error serializing verdict: {e}"))?;
    std::fs::write(&verdict_path, json_str + "\n")
        .map_err(|e| format!("error writing {}: {e}", verdict_path.display()))?;
    crate::clean::clean_on_review(root, id);
    Ok(())
}

pub fn read_verdict(root: &Path, id: &str) -> Result<Option<ReviewVerdict>, String> {
    let verdict_path = root.join(".wt").join(format!("{id}.verdict.json"));
    let raw = match std::fs::read_to_string(&verdict_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!("error reading {}: {e}", verdict_path.display()));
        }
    };
    let val: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("could not parse {}: {e}", verdict_path.display()))?;
    let verdict_str = val
        .get("verdict")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} is missing string verdict", verdict_path.display()))?;
    let parsed = match verdict_str {
        "accepted" => ReviewVerdict::Accepted,
        "fixed" => ReviewVerdict::Fixed,
        "redone" => ReviewVerdict::Redone,
        other => {
            return Err(format!(
                "{} has unknown verdict '{other}'",
                verdict_path.display()
            ));
        }
    };
    Ok(Some(parsed))
}

enum ReviewError {
    WorktreeGone(String),
    Other(String),
}

impl ReviewError {
    fn into_message(self) -> String {
        match self {
            Self::WorktreeGone(msg) | Self::Other(msg) => msg,
        }
    }
}

enum WtGit {
    Output(std::process::Output),
    WorktreeGone,
    IoErr(std::io::Error),
}

fn wt_present(wt: &Path) -> bool {
    wt.is_dir() && wt.join(".git").exists()
}

fn run_git_in_wt(wt: &Path, args: &[&str]) -> WtGit {
    match Command::new("git").args(args).current_dir(wt).output() {
        Ok(o) => {
            if !o.status.success() && !wt_present(wt) {
                WtGit::WorktreeGone
            } else {
                WtGit::Output(o)
            }
        }
        Err(e) => {
            if !wt_present(wt) {
                WtGit::WorktreeGone
            } else {
                WtGit::IoErr(e)
            }
        }
    }
}

struct WorktreeGitData {
    base: String,
    start_comments: Option<String>,
    commits_out: String,
    files_out: String,
    tree_out: String,
    diff_raw: Option<String>,
    diff_out: String,
    task_text: Option<String>,
    changed_files: Vec<String>,
    subjects: Vec<String>,
}

impl WorktreeGitData {
    fn empty() -> Self {
        Self {
            base: String::new(),
            start_comments: None,
            commits_out: String::new(),
            files_out: String::new(),
            tree_out: String::new(),
            diff_raw: None,
            diff_out: String::new(),
            task_text: None,
            changed_files: Vec::new(),
            subjects: Vec::new(),
        }
    }
}

fn git_stdout_in_wt_with(
    run_git: &dyn Fn(&Path, &[&str]) -> WtGit,
    wt: &Path,
    args: &[&str],
) -> Result<Option<String>, String> {
    match run_git(wt, args) {
        WtGit::WorktreeGone => Ok(None),
        WtGit::IoErr(e) => Err(format!(
            "could not run git {} in {}: {e}",
            args.join(" "),
            wt.display()
        )),
        WtGit::Output(o) if o.status.success() => {
            Ok(Some(String::from_utf8_lossy(&o.stdout).into_owned()))
        }
        WtGit::Output(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("exit {}", o.status)
            } else {
                stderr
            };
            Err(format!(
                "git {} failed in {}: {detail}",
                args.join(" "),
                wt.display()
            ))
        }
    }
}

fn read_worktree_git_data(
    root: &Path,
    id: &str,
    wt: &Path,
    is_research: bool,
    include_diff_for_research: bool,
    before_git: Option<&dyn Fn()>,
    run_git: &dyn Fn(&Path, &[&str]) -> WtGit,
) -> Result<Option<WorktreeGitData>, String> {
    let start_path = root.join(".wt").join(format!("{id}.start.json"));
    let (start_base_sha, start_comments): (Option<String>, Option<String>) =
        match std::fs::read_to_string(&start_path) {
            Ok(c) => match serde_json::from_str::<serde_json::Value>(&c) {
                Ok(v) => (
                    v.get("base_sha")
                        .and_then(|s| s.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                    v.get("comments")
                        .and_then(|s| s.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                ),
                Err(e) => {
                    return Err(format!("cannot parse {}: {e}", start_path.display()));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(e) => {
                return Err(format!("cannot read {}: {e}", start_path.display()));
            }
        };

    if let Some(hook) = before_git {
        hook();
    }

    let base = if let Some(sha) = start_base_sha {
        let out = Command::new("git")
            .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
            .current_dir(root)
            .output()
            .map_err(|e| format!("cannot run git cat-file in {}: {e}", root.display()))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(format!(
                "run {id} started from {sha}, which is not in this repository (git cat-file in {}: {stderr})",
                root.display()
            ));
        }
        sha
    } else {
        let root_head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output();
        let root_head_sha = match root_head {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => {
                return Err("could not resolve root HEAD".to_string());
            }
        };

        match run_git(wt, &["merge-base", "HEAD", &root_head_sha]) {
            WtGit::WorktreeGone => return Ok(None),
            WtGit::IoErr(e) => {
                return Err(format!(
                    "could not run git merge-base HEAD {root_head_sha} in {}: {e}",
                    wt.display()
                ));
            }
            WtGit::Output(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            WtGit::Output(_) => {
                return Err(format!(
                    "cannot find where run {id} started; its start record has no base_sha and the worktree shares no history with {root_head_sha}"
                ));
            }
        }
    };

    let range = format!("{base}..HEAD");

    let Some(commits_raw) = git_stdout_in_wt_with(run_git, wt, &["log", "--oneline", &range])?
    else {
        return Ok(None);
    };
    let commits_out = commits_raw.trim().to_string();

    let Some(files_raw) = git_stdout_in_wt_with(run_git, wt, &["diff", "--stat", &range])? else {
        return Ok(None);
    };
    let files_out = files_raw.trim().to_string();

    let Some(tree_raw) = git_stdout_in_wt_with(run_git, wt, &["status", "--short"])? else {
        return Ok(None);
    };
    let tree_out = tree_raw.trim().to_string();

    let diff_raw = if !is_research || include_diff_for_research {
        let Some(diff) = git_stdout_in_wt_with(run_git, wt, &["diff", &range])? else {
            return Ok(None);
        };
        Some(diff)
    } else {
        None
    };

    let diff_out = match diff_raw.as_deref() {
        Some(s) => s.trim().to_string(),
        None => String::new(),
    };

    let task_text: Option<String> = (|| {
        let start_file = root.join(".wt").join(format!("{id}.start.json"));
        let content = std::fs::read_to_string(&start_file).ok()?;
        let val: serde_json::Value = serde_json::from_str(&content).ok()?;
        let task_file = val.get("task_file")?.as_str()?;
        let p = Path::new(task_file);
        if let Ok(s) = std::fs::read_to_string(p) {
            return Some(s);
        }
        std::fs::read_to_string(root.join(p)).ok()
    })();

    let Some(changed_raw) = git_stdout_in_wt_with(run_git, wt, &["diff", "--name-only", &range])?
    else {
        return Ok(None);
    };
    let changed_files: Vec<String> = changed_raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let Some(subjects_raw) = git_stdout_in_wt_with(run_git, wt, &["log", "--format=%s", &range])?
    else {
        return Ok(None);
    };
    let subjects: Vec<String> = subjects_raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(Some(WorktreeGitData {
        base,
        start_comments,
        commits_out,
        files_out,
        tree_out,
        diff_raw,
        diff_out,
        task_text,
        changed_files,
        subjects,
    }))
}

pub(crate) fn compute_review(
    root: &Path,
    id: &str,
    include_diff_for_research: bool,
) -> Result<ReviewComputation, String> {
    compute_review_res(root, id, include_diff_for_research, None, None)
        .map_err(ReviewError::into_message)
}

fn compute_review_res(
    root: &Path,
    id: &str,
    include_diff_for_research: bool,
    summary_worktree_fallback: Option<&Path>,
    before_git: Option<&dyn Fn()>,
) -> Result<ReviewComputation, ReviewError> {
    compute_review_res_with_runner(
        root,
        id,
        include_diff_for_research,
        summary_worktree_fallback,
        before_git,
        &run_git_in_wt,
    )
}

fn compute_review_res_with_runner(
    root: &Path,
    id: &str,
    include_diff_for_research: bool,
    summary_worktree_fallback: Option<&Path>,
    before_git: Option<&dyn Fn()>,
    run_git: &dyn Fn(&Path, &[&str]) -> WtGit,
) -> Result<ReviewComputation, ReviewError> {
    let wt = root.join(".wt").join(id);
    let mut wt_exists = wt_present(&wt);
    let is_research = crate::clean::is_research(root, id)
        || root.join(".wt").join(format!("{id}.report.md")).exists();
    let report_file = root.join(".wt").join(format!("{id}.report.json"));
    let summary_file = root.join(".wt").join(format!("{id}.json"));
    let missing_wt_err = |wt: &Path| -> Option<ReviewError> {
        if !is_research {
            if !report_file.exists() && summary_file.exists() {
                return None;
            }
            return Some(ReviewError::WorktreeGone(format!(
                "worktree {} does not exist",
                wt.display()
            )));
        }
        if !report_file.exists() && !summary_file.exists() {
            return Some(ReviewError::WorktreeGone(format!(
                "worktree {} does not exist and {} is missing: nothing to review",
                wt.display(),
                report_file.display()
            )));
        }
        None
    };
    if !wt_exists {
        if let Some(err) = missing_wt_err(&wt) {
            return Err(err);
        }
    }

    let git_data = if wt_exists {
        match read_worktree_git_data(
            root,
            id,
            &wt,
            is_research,
            include_diff_for_research,
            before_git,
            run_git,
        ) {
            Ok(Some(data)) => data,
            Ok(None) => {
                wt_exists = false;
                if let Some(err) = missing_wt_err(&wt) {
                    return Err(err);
                }
                WorktreeGitData::empty()
            }
            Err(e) => return Err(ReviewError::Other(e)),
        }
    } else {
        WorktreeGitData::empty()
    };

    let WorktreeGitData {
        base,
        start_comments,
        commits_out,
        files_out,
        tree_out,
        diff_raw,
        diff_out,
        task_text,
        changed_files,
        subjects,
    } = git_data;

    let log_file = root.join(".wt").join(format!("{id}.log"));
    let log_content_opt = match std::fs::read_to_string(&log_file) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(ReviewError::Other(format!(
                "cannot read {}: {e}",
                log_file.display()
            )));
        }
    };
    let (hcmds, reads, (refused_count, refused_paths)) =
        if let Some(ref log_content) = log_content_opt {
            let hcmds = host_commands(log_content);
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            let allowed = [root.to_path_buf(), wt.clone()];
            let reads = outside_reads(log_content, &allowed, &home);
            let refused = refused_reads(log_content);
            (hcmds, reads, refused)
        } else {
            (Vec::new(), Vec::new(), (0, Vec::new()))
        };

    let report_val = match read_optional_json(&report_file)? {
        Some(v) => Some(v),
        None => read_optional_json(&summary_file)?,
    };
    let outcome = report_val
        .as_ref()
        .and_then(|v| v.get("outcome"))
        .and_then(|x| x.as_str());
    let quote_problems: Option<Vec<String>> = report_val
        .as_ref()
        .and_then(|v| v.get("quote_problems"))
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        });
    let read_loop = report_val
        .as_ref()
        .and_then(|v| v.get("read_loop"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let gate_left_out: Vec<String> = report_val
        .as_ref()
        .and_then(|v| v.get("gate_left_out"))
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let start_file = root.join(".wt").join(format!("{id}.start.json"));
            let content = std::fs::read_to_string(&start_file).ok()?;
            let val: serde_json::Value = serde_json::from_str(&content).ok()?;
            let arr = val.get("gate_left_out")?.as_array()?;
            Some(
                arr.iter()
                    .filter_map(|item| {
                        let cmd = item.get("command")?.as_str()?;
                        let reason = item.get("reason")?.as_str()?;
                        Some(format!("{cmd} ({reason})"))
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();
    let claim_check: Option<blyte_agent::claim_check::ClaimCheck> = report_val
        .as_ref()
        .and_then(|v| v.get("claim_check"))
        .and_then(|x| serde_json::from_value(x.clone()).ok());
    let claim_check_skipped: Option<String> = report_val
        .as_ref()
        .and_then(|v| v.get("claim_check_skipped"))
        .and_then(|x| x.as_str())
        .map(String::from);

    let mut checks = if wt_exists {
        crate::review_checks::build(
            task_text.as_deref(),
            &changed_files,
            &subjects,
            hcmds.len(),
            reads.len(),
            outcome,
        )
    } else {
        crate::review_checks::build(
            Some(""),
            &[String::new()],
            &[],
            hcmds.len(),
            reads.len(),
            outcome,
        )
    };
    let mut fallbacks: Vec<crate::fallback_check::Finding> = Vec::new();
    if !is_research {
        if let Some(ref diff) = diff_raw {
            fallbacks = crate::fallback_check::added_fallbacks(diff);
            if start_comments.as_deref() != Some("off") {
                let wt_gone = std::cell::Cell::new(false);
                match crate::comment_lint::added_comments(diff, &|path| match run_git_in_wt(
                    &wt,
                    &["show", &format!("HEAD:{path}")],
                ) {
                    WtGit::WorktreeGone => {
                        wt_gone.set(true);
                        Err(format!("worktree {} does not exist", wt.display()))
                    }
                    WtGit::IoErr(e) => Err(e.to_string()),
                    WtGit::Output(o) => {
                        if o.status.success() {
                            Ok(String::from_utf8_lossy(&o.stdout).into_owned())
                        } else {
                            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                            if stderr.is_empty() {
                                Err(format!("exit {}", o.status))
                            } else {
                                Err(stderr)
                            }
                        }
                    }
                }) {
                    Ok(comments) => {
                        if let Some(flag) = crate::comment_lint::flag(&comments) {
                            checks.flags.push(flag);
                        }
                    }
                    Err(reason) => {
                        if wt_gone.get() {
                            return Err(ReviewError::WorktreeGone(format!(
                                "worktree {} does not exist",
                                wt.display()
                            )));
                        }
                        let first_line = reason.lines().next().unwrap_or(reason.as_str());
                        let msg = format!("comment check not run: {first_line}");
                        let cut: String = msg.chars().take(200).collect();
                        checks.flags.push(cut);
                    }
                }
            }
        }
        if wt_exists {
            match std::fs::read_to_string(&summary_file) {
                Ok(c) => match serde_json::from_str::<serde_json::Value>(&c) {
                    Ok(v) => match crate::outside_env::summary_worktree(&v) {
                        Ok(summary_wt) => {
                            if let Some(ref log_content) = log_content_opt {
                                let borrowed = crate::outside_env::borrowed_env_commands(
                                    log_content,
                                    &summary_wt,
                                );
                                if let Some(flag) = crate::outside_env::flag(&borrowed) {
                                    checks.flags.push(flag);
                                }
                            }
                        }
                        Err(e) => {
                            checks.flags.push(format!("outside-env check not run: {e}"));
                        }
                    },
                    Err(e) => {
                        checks.flags.push(format!(
                            "outside-env check not run: cannot parse {}: {e}",
                            summary_file.display()
                        ));
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && log_content_opt.is_none() => {
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && summary_worktree_fallback.is_some() =>
                {
                    if let (Some(summary_wt), Some(ref log_content)) =
                        (summary_worktree_fallback, &log_content_opt)
                    {
                        let borrowed =
                            crate::outside_env::borrowed_env_commands(log_content, summary_wt);
                        if let Some(flag) = crate::outside_env::flag(&borrowed) {
                            checks.flags.push(flag);
                        }
                    }
                }
                Err(e) => {
                    checks.flags.push(format!(
                        "outside-env check not run: cannot read {}: {e}",
                        summary_file.display()
                    ));
                }
            }
        }
    }
    if let Some(ref problems) = quote_problems {
        if !problems.is_empty() {
            checks.flags.push(format!(
                "quote check failed ({} problem(s)): see == report",
                problems.len()
            ));
        }
    }
    if read_loop {
        checks.flags.push(
            "read loop: the run kept re-reading the same files without changing anything and was stopped"
                .to_string(),
        );
    }
    if !gate_left_out.is_empty() {
        checks
            .flags
            .push(format!("gate left out: {}", gate_left_out.join("; ")));
    }
    if let Some(ref cc) = claim_check {
        if !cc.contradicted.is_empty() {
            checks.flags.push(format!(
                "claim check: {} contradicted claims",
                cc.contradicted.len()
            ));
        }
    }
    if let Some(ref skipped) = claim_check_skipped {
        checks.flags.push(format!("claim check skipped: {skipped}"));
    }
    if refused_count > 0 {
        let first_3: Vec<&str> = refused_paths.iter().take(3).map(|s| s.as_str()).collect();
        checks.flags.push(format!(
            "refused reads: {refused_count} (outside the workspace and --allow-read): {}",
            first_3.join(", ")
        ));
    }
    if is_research {
        match &report_val {
            Some(rv) => match crate::research_breadth::large_prompt_turns(rv) {
                Ok((large, total)) => {
                    if let Some(flag) = crate::research_breadth::breadth_flag(large, total) {
                        checks.flags.push(flag);
                    }
                }
                Err(e) => {
                    checks.flags.push(format!("breadth check not run: {e}"));
                }
            },
            None => {
                checks.flags.push(format!(
                    "breadth check not run: cannot read {}",
                    report_file.display()
                ));
            }
        }
        if outcome == Some("budget_exhausted") {
            match std::fs::read_to_string(&log_file) {
                Ok(log) => {
                    let dirs = crate::research_breadth::read_dirs(&log, root);
                    if let Some(flag) = crate::research_breadth::split_flag(&dirs) {
                        checks.flags.push(flag);
                    }
                }
                Err(e) => {
                    checks.flags.push(format!("split suggestion not run: {e}"));
                }
            }
        }
    }

    Ok(ReviewComputation {
        wt,
        wt_exists,
        is_research,
        base,
        task_text,
        changed_files,
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
        checks,
        fallbacks,
    })
}

pub fn read_stored_model_review(
    root: &Path,
    id: &str,
) -> Result<Option<crate::model_review::ModelReviewResult>, String> {
    let checks_path = root.join(".wt").join(format!("{id}.checks.json"));
    let raw = match std::fs::read_to_string(&checks_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", checks_path.display())),
    };
    let val: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse {}: {e}", checks_path.display()))?;
    parse_stored_model_review(&checks_path, &val)
}

pub(crate) fn parse_stored_model_review(
    checks_path: &Path,
    val: &serde_json::Value,
) -> Result<Option<crate::model_review::ModelReviewResult>, String> {
    let Some(obj) = val.as_object() else {
        return Err(format!(
            "cannot parse {}: expected a JSON object",
            checks_path.display()
        ));
    };
    let Some(mr) = obj.get("model_review") else {
        return Ok(None);
    };
    if mr.is_null() {
        return Ok(None);
    }
    let parsed: crate::model_review::ModelReviewResult = serde_json::from_value(mr.clone())
        .map_err(|e| format!("invalid model_review in {}: {e}", checks_path.display()))?;
    parsed
        .validate()
        .map_err(|e| format!("invalid model_review in {}: {e}", checks_path.display()))?;
    Ok(Some(parsed))
}

fn read_optional_json(path: &Path) -> Result<Option<serde_json::Value>, ReviewError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ReviewError::Other(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .map(Some)
        .map_err(|e| ReviewError::Other(format!("cannot parse {}: {e}", path.display())))
}

pub(crate) fn write_checks_file(
    root: &Path,
    id: &str,
    flags: &[String],
    fallbacks: &[crate::fallback_check::Finding],
    model_review: Option<&crate::model_review::ModelReviewResult>,
) -> Result<(), String> {
    let checks_path = root.join(".wt").join(format!("{id}.checks.json"));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("error: the system clock is before 1970: {e}"))?
        .as_secs();
    let annotated_fallbacks = crate::fallback_check::annotate_fallbacks(fallbacks, model_review);
    let mut checks_obj = serde_json::json!({
        "ts": ts,
        "flags": flags,
        "fallbacks": annotated_fallbacks,
    });
    if let Some(mr) = model_review {
        checks_obj["model_review"] = serde_json::to_value(mr).map_err(|e| {
            format!(
                "error serializing model review for {}: {e}",
                checks_path.display()
            )
        })?;
    }
    let checks_json = serde_json::to_string_pretty(&checks_obj)
        .map_err(|e| format!("error writing {}: {e}", checks_path.display()))?;
    std::fs::write(&checks_path, checks_json + "\n")
        .map_err(|e| format!("error writing {}: {e}", checks_path.display()))
}

pub fn evaluate_verified_run(
    root: &Path,
    id: &str,
) -> Result<Option<crate::model_review::ModelReviewResult>, String> {
    evaluate_verified_run_with(root, id, false, crate::model_review::run)
}

pub fn evaluate_verified_run_with<F>(
    root: &Path,
    id: &str,
    force_review: bool,
    run_model_review: F,
) -> Result<Option<crate::model_review::ModelReviewResult>, String>
where
    F: FnOnce(&Path, &str, &str, &Path) -> crate::model_review::ModelReviewResult,
{
    let wt_fallback = root.join(".wt").join(id);
    let data = compute_review_res(root, id, false, Some(&wt_fallback), None)
        .map_err(ReviewError::into_message)?;
    let mut flags = data.checks.flags;
    if data.is_research || !data.wt_exists {
        flags.extend(crate::fallback_check::flags_for_review(
            &data.fallbacks,
            None,
        ));
        write_checks_file(root, id, &flags, &data.fallbacks, None)?;
        return Ok(None);
    }

    let risky_matches = match crate::config::load_with_fallback(root) {
        Ok(Some((cfg, _))) => {
            match crate::config::match_risky_paths(&cfg.review.risky, &data.changed_files) {
                Ok(matches) => matches,
                Err(e) => {
                    flags.push(format!("risky path check not run: {e}"));
                    Vec::new()
                }
            }
        }
        Ok(None) => Vec::new(),
        Err(e) => {
            flags.push(format!("risky path check not run: {e}"));
            Vec::new()
        }
    };

    if force_review
        || crate::review_checks::should_run_model_review(
            &flags,
            data.task_text.as_deref(),
            &risky_matches,
            data.fallbacks.len(),
        )
    {
        let result = run_model_review(root, id, &data.base, &data.wt);
        flags.extend(crate::fallback_check::flags_for_review(
            &data.fallbacks,
            Some(&result),
        ));
        if let Some(flag) = result.flag() {
            flags.push(flag);
        }
        write_checks_file(root, id, &flags, &data.fallbacks, Some(&result))?;
        Ok(Some(result))
    } else {
        let review_path = root.join(".wt").join(format!("{id}.model-review.json"));
        if review_path.exists() {
            std::fs::remove_file(&review_path)
                .map_err(|e| format!("cannot remove {}: {e}", review_path.display()))?;
        }
        flags.extend(crate::fallback_check::flags_for_review(
            &data.fallbacks,
            None,
        ));
        write_checks_file(root, id, &flags, &data.fallbacks, None)?;
        Ok(None)
    }
}

pub fn ensure_checks(root: &Path, id: &str) -> Result<bool, String> {
    ensure_checks_with_hook(root, id, None)
}

fn ensure_checks_with_hook(
    root: &Path,
    id: &str,
    before_git: Option<&dyn Fn()>,
) -> Result<bool, String> {
    let wt_dir = root.join(".wt");
    let summary_path = wt_dir.join(format!("{id}.json"));
    if !summary_path.is_file() {
        return Err(format!("no finished run {id} in {}", wt_dir.display()));
    }
    let checks_path = wt_dir.join(format!("{id}.checks.json"));
    if checks_path.exists() {
        return Ok(false);
    }
    let data = match compute_review_res(root, id, false, None, before_git) {
        Ok(data) => data,
        Err(ReviewError::WorktreeGone(_)) => return Ok(false),
        Err(ReviewError::Other(e)) => return Err(e),
    };
    if !data.wt_exists {
        return Ok(false);
    }
    let mut flags = data.checks.flags;
    flags.extend(crate::fallback_check::flags_for_review(
        &data.fallbacks,
        None,
    ));
    write_checks_file(root, id, &flags, &data.fallbacks, None)?;
    Ok(true)
}

pub fn set_hold(root: &Path, id: &str, note: &str, now: u64) -> Result<(), String> {
    let trimmed = note.trim();
    if trimmed.is_empty() {
        return Err(format!("a note is required to hold run {id}"));
    }
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir)
        .map_err(|e| format!("error creating directory {}: {e}", wt_dir.display()))?;
    let hold_path = wt_dir.join(format!("{id}.hold.json"));
    let hold = Hold {
        note: trimmed.to_string(),
        ts: now,
    };
    let json_str =
        serde_json::to_string_pretty(&hold).map_err(|e| format!("error serializing hold: {e}"))?;
    std::fs::write(&hold_path, json_str + "\n")
        .map_err(|e| format!("error writing {}: {e}", hold_path.display()))
}

pub fn clear_hold(root: &Path, id: &str) -> Result<(), String> {
    let hold_path = root.join(".wt").join(format!("{id}.hold.json"));
    match std::fs::remove_file(&hold_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("error removing {}: {e}", hold_path.display())),
    }
}

pub fn read_hold(root: &Path, id: &str) -> Result<Option<Hold>, String> {
    let hold_path = root.join(".wt").join(format!("{id}.hold.json"));
    let raw = match std::fs::read_to_string(&hold_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("error reading {}: {e}", hold_path.display())),
    };
    serde_json::from_str::<Hold>(&raw)
        .map(Some)
        .map_err(|e| format!("could not parse {}: {e}", hold_path.display()))
}

pub fn decide(
    root: &Path,
    id: &str,
    action: Action,
    note: Option<&str>,
    now: u64,
) -> Result<Decision, String> {
    let wt_dir = root.join(".wt");
    let summary_path = wt_dir.join(format!("{id}.json"));
    let summary_raw = match std::fs::read_to_string(&summary_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("no finished run {id} in {}", wt_dir.display()));
        }
        Err(e) => {
            return Err(format!("error reading {}: {e}", summary_path.display()));
        }
    };
    let summary: serde_json::Value = serde_json::from_str(&summary_raw)
        .map_err(|e| format!("could not parse {}: {e}", summary_path.display()))?;
    let outcome = summary
        .get("outcome")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} has no outcome", summary_path.display()))?;
    let is_research =
        crate::clean::is_research(root, id) || wt_dir.join(format!("{id}.report.md")).exists();

    let trimmed_note: Option<String> = note
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let conversation_message = match action {
        Action::Accept => {
            if is_research {
                return Err(format!("run {id} is a research run; mark it read"));
            }
            if outcome != "verified" {
                return Err(format!(
                    "run {id} is {outcome}, not verified; it cannot be accepted"
                ));
            }
            record_verdict(
                root,
                id,
                ReviewVerdict::Accepted,
                trimmed_note.as_deref().unwrap_or(""),
                now,
            )?;
            clear_hold(root, id)?;
            None
        }
        Action::SendBack => {
            let Some(ref text) = trimmed_note else {
                return Err(format!("a note is required to send back run {id}"));
            };
            clear_hold(root, id)?;
            Some(format!(
                "Send back run {id}: {text} Draft a follow-up task for me to confirm."
            ))
        }
        Action::Redo => {
            record_verdict(
                root,
                id,
                ReviewVerdict::Redone,
                trimmed_note.as_deref().unwrap_or(""),
                now,
            )?;
            clear_hold(root, id)?;
            let msg = match trimmed_note.as_deref() {
                Some(text) => format!("Redo run {id}. Reason: {text}"),
                None => format!("Redo run {id}."),
            };
            Some(msg)
        }
        Action::Hold => {
            let Some(ref text) = trimmed_note else {
                return Err(format!("a note is required to hold run {id}"));
            };
            set_hold(root, id, text, now)?;
            None
        }
        Action::MarkRead => {
            if !is_research {
                return Err(format!("run {id} is not a research run"));
            }
            record_verdict(root, id, ReviewVerdict::Accepted, "read", now)?;
            clear_hold(root, id)?;
            None
        }
    };

    Ok(Decision {
        run: id.to_string(),
        action,
        note: trimmed_note,
        conversation_message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]);
        std::fs::write(d.path().join("f.txt"), "hello\n").unwrap();
        git(d.path(), &["add", "."]);
        git(d.path(), &["commit", "-qm", "init", "--no-gpg-sign"]);
        std::fs::create_dir_all(d.path().join(".wt")).unwrap();
        let head = git(d.path(), &["rev-parse", "HEAD"]);
        (d, head)
    }

    fn setup_finished_code_run(root: &Path, id: &str, base_sha: Option<&str>) {
        let wt_path = root.join(".wt").join(id);
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("wip/{id}"),
                wt_path.to_str().unwrap(),
                "HEAD",
            ],
        );
        let mut start = serde_json::json!({
            "id": id,
            "pid": 999999,
            "started_at": 1000,
            "research": false,
        });
        if let Some(sha) = base_sha {
            start["base_sha"] = serde_json::json!(sha);
        }
        std::fs::write(
            root.join(".wt").join(format!("{id}.start.json")),
            start.to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join(".wt").join(format!("{id}.report.json")),
            serde_json::json!({ "outcome": "error", "turns": 0 }).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join(".wt").join(format!("{id}.json")),
            serde_json::json!({
                "outcome": "error",
                "seconds": 1,
                "final_message": "",
                "commits": [],
                "changed_files": []
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn ensure_checks_skips_when_worktree_vanishes_before_git_commands() {
        let (d, head) = init_repo();
        setup_finished_code_run(d.path(), "M2", Some(&head));
        let root = d.path().to_path_buf();
        let remove_wt = move || {
            let wt = root.join(".wt/M2");
            let status = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(&wt)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success());
        };

        let wrote = ensure_checks_with_hook(d.path(), "M2", Some(&remove_wt)).unwrap();
        assert!(!wrote);
        assert!(!d.path().join(".wt/M2").exists());
        assert!(!d.path().join(".wt/M2.checks.json").exists());
    }

    #[test]
    fn ensure_checks_skips_when_worktree_vanishes_without_base_sha() {
        let (d, _head) = init_repo();
        setup_finished_code_run(d.path(), "M3", None);
        let root = d.path().to_path_buf();
        let remove_wt = move || {
            let wt = root.join(".wt/M3");
            let status = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(&wt)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success());
        };

        let wrote = ensure_checks_with_hook(d.path(), "M3", Some(&remove_wt)).unwrap();
        assert!(!wrote);
        assert!(!d.path().join(".wt/M3.checks.json").exists());
    }

    #[test]
    fn ensure_checks_still_errors_when_base_sha_is_not_in_repository() {
        let (d, _head) = init_repo();
        let missing = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        setup_finished_code_run(d.path(), "BAD1", Some(missing));

        let err = ensure_checks(d.path(), "BAD1").unwrap_err();
        assert!(
            err.contains(&format!(
                "run BAD1 started from {missing}, which is not in this repository"
            )),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn research_review_uses_no_worktree_path_when_worktree_vanishes_mid_read() {
        let (d, _head) = init_repo();
        let wt = d.path().join(".wt/R1");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q"]);
        git(
            &wt,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "research",
                "--no-gpg-sign",
            ],
        );
        std::fs::write(
            d.path().join(".wt/R1.start.json"),
            serde_json::json!({ "id": "R1", "research": true }).to_string(),
        )
        .unwrap();
        std::fs::write(
            d.path().join(".wt/R1.report.json"),
            serde_json::json!({ "outcome": "verified", "turns": 1 }).to_string(),
        )
        .unwrap();

        let wt_to_remove = wt.clone();
        let remove_wt = move || {
            std::fs::remove_dir_all(&wt_to_remove).unwrap();
        };
        let data = compute_review_res(d.path(), "R1", false, None, Some(&remove_wt))
            .map_err(ReviewError::into_message)
            .unwrap();
        assert!(!data.wt_exists);
        assert!(data.is_research);
    }

    #[test]
    fn review_summary_errors_when_worktree_git_io_fails_naming_args() {
        let (d, head) = init_repo();
        setup_finished_code_run(d.path(), "G1", Some(&head));
        let wt = d.path().join(".wt/G1");
        let range = format!("{head}..HEAD");

        let failing_calls: &[(&[&str], &str)] = &[
            (&["log", "--oneline"], "log --oneline"),
            (&["diff", "--stat"], "diff --stat"),
            (&["status", "--short"], "status --short"),
            (&["diff", range.as_str()], "diff"),
            (&["diff", "--name-only"], "diff --name-only"),
            (&["log", "--format=%s"], "log --format=%s"),
        ];

        for &(fail_prefix, expected_args) in failing_calls {
            let runner = |dir: &Path, args: &[&str]| -> WtGit {
                if args.starts_with(fail_prefix) {
                    WtGit::IoErr(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "git binary missing",
                    ))
                } else {
                    run_git_in_wt(dir, args)
                }
            };
            let err = compute_review_res_with_runner(d.path(), "G1", false, None, None, &runner)
                .err()
                .expect("expected error")
                .into_message();
            assert!(
                err.contains("could not run git"),
                "for {expected_args}, got: {err}"
            );
            assert!(
                err.contains(expected_args),
                "for {expected_args}, got: {err}"
            );
            assert!(
                err.contains(&wt.display().to_string()),
                "for {expected_args}, got: {err}"
            );
            assert!(
                err.contains("git binary missing"),
                "for {expected_args}, got: {err}"
            );
        }
    }

    #[test]
    fn review_summary_errors_when_worktree_git_exits_nonzero_with_stderr() {
        let (d, head) = init_repo();
        setup_finished_code_run(d.path(), "G2", Some(&head));

        let runner = |dir: &Path, args: &[&str]| -> WtGit {
            if args.starts_with(&["diff", "--stat"]) {
                let mut out = Command::new("sh")
                    .args(["-c", "echo 'fatal: bad revision' >&2; exit 128"])
                    .output()
                    .unwrap();
                out.stdout.clear();
                WtGit::Output(out)
            } else {
                run_git_in_wt(dir, args)
            }
        };
        let err = compute_review_res_with_runner(d.path(), "G2", false, None, None, &runner)
            .err()
            .expect("expected error")
            .into_message();
        assert!(err.contains("diff --stat"), "got: {err}");
        assert!(err.contains("fatal: bad revision"), "got: {err}");
    }

    #[test]
    fn compute_review_errors_on_corrupt_report_json_naming_file() {
        let (d, head) = init_repo();
        setup_finished_code_run(d.path(), "C1", Some(&head));
        let report_path = d.path().join(".wt/C1.report.json");
        std::fs::write(&report_path, "{corrupt json").unwrap();

        let err = compute_review(d.path(), "C1", false)
            .err()
            .expect("expected error for corrupt report.json");
        assert!(
            err.contains(&report_path.display().to_string()),
            "got: {err}"
        );
    }

    #[test]
    fn compute_review_errors_on_unreadable_log_directory_naming_file() {
        let (d, head) = init_repo();
        setup_finished_code_run(d.path(), "C2", Some(&head));
        let log_path = d.path().join(".wt/C2.log");
        std::fs::create_dir_all(&log_path).unwrap();

        let err = compute_review(d.path(), "C2", false)
            .err()
            .expect("expected error for unreadable log directory");
        assert!(err.contains(&log_path.display().to_string()), "got: {err}");
    }

    #[test]
    fn compute_review_errors_on_corrupt_summary_json_when_report_absent() {
        let (d, head) = init_repo();
        setup_finished_code_run(d.path(), "C3", Some(&head));
        std::fs::remove_file(d.path().join(".wt/C3.report.json")).unwrap();
        let summary_path = d.path().join(".wt/C3.json");
        std::fs::write(&summary_path, "not json at all").unwrap();

        let err = compute_review(d.path(), "C3", false)
            .err()
            .expect("expected error for corrupt summary json");
        assert!(
            err.contains(&summary_path.display().to_string()),
            "got: {err}"
        );
    }
}
