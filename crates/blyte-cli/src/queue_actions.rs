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
    pub orchestrator_message: Option<String>,
}

pub(crate) struct ReviewComputation {
    pub(crate) wt: PathBuf,
    pub(crate) wt_exists: bool,
    pub(crate) base: String,
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

pub(crate) fn compute_review(
    root: &Path,
    id: &str,
    include_diff_for_research: bool,
) -> Result<ReviewComputation, String> {
    let wt = root.join(".wt").join(id);
    let wt_exists = wt.exists();
    let is_research = crate::clean::is_research(root, id)
        || root.join(".wt").join(format!("{id}.report.md")).exists();
    let report_file = root.join(".wt").join(format!("{id}.report.json"));
    if !wt_exists {
        if !is_research {
            return Err(format!("worktree {} does not exist", wt.display()));
        }
        if !report_file.exists() {
            return Err(format!(
                "worktree {} does not exist and {} is missing: nothing to review",
                wt.display(),
                report_file.display()
            ));
        }
    }

    let (
        base,
        start_comments,
        commits_out,
        files_out,
        tree_out,
        diff_res,
        diff_out,
        task_text,
        changed_files,
        subjects,
    ) = if wt_exists {
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

        let base = if let Some(sha) = start_base_sha {
            let has_base = Command::new("git")
                .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
                .current_dir(&wt)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !has_base {
                return Err(format!(
                    "run {id} started from {sha}, which is not in this repository"
                ));
            }
            sha
        } else {
            let root_head = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output();
            let root_head_sha = match root_head {
                Ok(o) if o.status.success() => {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                }
                _ => {
                    return Err("could not resolve root HEAD".to_string());
                }
            };

            let mb_out = Command::new("git")
                .args(["merge-base", "HEAD", &root_head_sha])
                .current_dir(&wt)
                .output();
            match mb_out {
                Ok(o) if o.status.success() => {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                }
                _ => {
                    return Err(format!(
                        "cannot find where run {id} started; its start record has no base_sha and the worktree shares no history with {root_head_sha}"
                    ));
                }
            }
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

        let diff_res: Option<Result<String, String>> = if !is_research {
            Some(
                Command::new("git")
                    .args(["diff", &format!("{base}..HEAD")])
                    .current_dir(&wt)
                    .output()
                    .map_err(|e| e.to_string())
                    .and_then(|o| {
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
                    }),
            )
        } else if include_diff_for_research {
            Some(
                Command::new("git")
                    .args(["diff", &format!("{base}..HEAD")])
                    .current_dir(&wt)
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .map_err(|e| e.to_string()),
            )
        } else {
            None
        };

        let diff_out = diff_res
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

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

        let changed_files: Vec<String> = Command::new("git")
            .args(["diff", "--name-only", &format!("{base}..HEAD")])
            .current_dir(&wt)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let subjects: Vec<String> = Command::new("git")
            .args(["log", "--format=%s", &format!("{base}..HEAD")])
            .current_dir(&wt)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        (
            base,
            start_comments,
            commits_out,
            files_out,
            tree_out,
            diff_res,
            diff_out,
            task_text,
            changed_files,
            subjects,
        )
    } else {
        (
            String::new(),
            None,
            String::new(),
            String::new(),
            String::new(),
            None,
            String::new(),
            None,
            Vec::new(),
            Vec::new(),
        )
    };

    let log_file = root.join(".wt").join(format!("{id}.log"));
    let log_content_opt = std::fs::read_to_string(&log_file).ok();
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

    let report_val = std::fs::read_to_string(&report_file)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok());
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
        if let Some(ref res) = diff_res {
            match res {
                Ok(diff) => {
                    let findings = crate::fallback_check::added_fallbacks(diff);
                    if let Some(flag) = crate::fallback_check::flag(&findings) {
                        checks.flags.push(flag);
                    }
                    fallbacks = findings;
                    if start_comments.as_deref() != Some("off") {
                        match crate::comment_lint::added_comments(diff, &|path| {
                            Command::new("git")
                                .args(["show", &format!("HEAD:{path}")])
                                .current_dir(&wt)
                                .output()
                                .map_err(|e| e.to_string())
                                .and_then(|o| {
                                    if o.status.success() {
                                        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
                                    } else {
                                        let stderr =
                                            String::from_utf8_lossy(&o.stderr).trim().to_string();
                                        if stderr.is_empty() {
                                            Err(format!("exit {}", o.status))
                                        } else {
                                            Err(stderr)
                                        }
                                    }
                                })
                        }) {
                            Ok(comments) => {
                                if let Some(flag) = crate::comment_lint::flag(&comments) {
                                    checks.flags.push(flag);
                                }
                            }
                            Err(reason) => {
                                let first_line = reason.lines().next().unwrap_or(reason.as_str());
                                let msg = format!("comment check not run: {first_line}");
                                let cut: String = msg.chars().take(200).collect();
                                checks.flags.push(cut);
                            }
                        }
                    }
                }
                Err(reason) => {
                    let first_line = reason.lines().next().unwrap_or(reason.as_str());
                    let msg = format!("fallback check not run: git diff failed: {first_line}");
                    let cut: String = msg.chars().take(200).collect();
                    checks.flags.push(cut);
                }
            }
        }
        let summary_path = root.join(".wt").join(format!("{id}.json"));
        match std::fs::read_to_string(&summary_path) {
            Ok(c) => match serde_json::from_str::<serde_json::Value>(&c) {
                Ok(v) => match crate::outside_env::summary_worktree(&v) {
                    Ok(summary_wt) => {
                        if let Some(ref log_content) = log_content_opt {
                            let borrowed =
                                crate::outside_env::borrowed_env_commands(log_content, &summary_wt);
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
                        summary_path.display()
                    ));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && log_content_opt.is_none() => {}
            Err(e) => {
                checks.flags.push(format!(
                    "outside-env check not run: cannot read {}: {e}",
                    summary_path.display()
                ));
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
        base,
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

pub(crate) fn write_checks_file(
    root: &Path,
    id: &str,
    flags: &[String],
    fallbacks: &[crate::fallback_check::Finding],
) -> Result<(), String> {
    let checks_path = root.join(".wt").join(format!("{id}.checks.json"));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("error: the system clock is before 1970: {e}"))?
        .as_secs();
    let checks_obj = serde_json::json!({
        "ts": ts,
        "flags": flags,
        "fallbacks": fallbacks,
    });
    let checks_json = serde_json::to_string_pretty(&checks_obj)
        .map_err(|e| format!("error writing {}: {e}", checks_path.display()))?;
    std::fs::write(&checks_path, checks_json + "\n")
        .map_err(|e| format!("error writing {}: {e}", checks_path.display()))
}

pub fn ensure_checks(root: &Path, id: &str) -> Result<bool, String> {
    let wt_dir = root.join(".wt");
    let summary_path = wt_dir.join(format!("{id}.json"));
    if !summary_path.is_file() {
        return Err(format!("no finished run {id} in {}", wt_dir.display()));
    }
    let checks_path = wt_dir.join(format!("{id}.checks.json"));
    if checks_path.exists() {
        return Ok(false);
    }
    let data = compute_review(root, id, false)?;
    write_checks_file(root, id, &data.checks.flags, &data.fallbacks)?;
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

    let orchestrator_message = match action {
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
            Some(format!(
                "Accepted run {id}. Combine it onto the working branch and run the gate."
            ))
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
        orchestrator_message,
    })
}
