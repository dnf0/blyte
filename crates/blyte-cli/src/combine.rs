//! `blyte combine <request>`: combine a request's passed runs on a work branch and run the gate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use blyte_agent::run::{gate_steps, run_gate_steps};
use blyte_agent::shell::{LocalShell, Output, Shell};
use serde::{Deserialize, Serialize};

use crate::cli::CombineArgs;

/// Final state recorded in `.wt/combined/<request>.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineState {
    Combined,
    Conflict,
    CheckFailed,
}

/// A run id and tip commit SHA included in a combine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombinedRun {
    pub id: String,
    pub tip: String,
}

/// Persistent combine record stored at `.wt/combined/<request>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombineRecord {
    pub request: String,
    pub state: CombineState,
    pub branch: String,
    pub base: String,
    pub runs: Vec<CombinedRun>,
    pub commits: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicting_run: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicting_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tail: Option<String>,
}

struct EnvShell<S> {
    inner: S,
    env: BTreeMap<String, String>,
}

impl<S: Shell> Shell for EnvShell<S> {
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        if self.env.is_empty() {
            return self.inner.exec(command, cwd, timeout);
        }
        let mut script = String::new();
        for (k, v) in &self.env {
            let quoted = format!("'{}'", v.replace('\'', "'\\''"));
            script.push_str(&format!("export {k}={quoted}\n"));
        }
        script.push_str(command);
        self.inner.exec(&script, cwd, timeout)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

#[derive(Debug, Clone)]
struct RunMeta {
    id: String,
    started_at: u64,
    base: Option<String>,
    base_sha: Option<String>,
    branch_hint: Option<String>,
}

/// Entry point for `blyte combine <request>`.
pub fn run(args: &CombineArgs) -> i32 {
    if let Err(msg) = crate::dispatch::validate_request_id(&args.request) {
        eprintln!("error: {msg}");
        return 2;
    }
    let root = match crate::clean::find_repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    combine_in(&root, &args.request, &mut stdout, &mut stderr)
}

/// Combine all passed non-research runs for `request` inside repository `root`.
pub fn combine_in(root: &Path, request: &str, out: &mut impl Write, err: &mut impl Write) -> i32 {
    if let Err(msg) = crate::dispatch::validate_request_id(request) {
        let _ = writeln!(err, "error: {msg}");
        return 2;
    }

    match run_combine_inner(root, request) {
        Ok(record) => {
            if let Ok(json) = serde_json::to_string_pretty(&record) {
                let _ = writeln!(out, "{json}");
            }
            match record.state {
                CombineState::Combined => 0,
                CombineState::Conflict => {
                    let run_id = record.conflicting_run.as_deref().unwrap_or("unknown");
                    let _ = writeln!(
                        err,
                        "combine conflict while applying run {run_id}: {}",
                        record.conflicting_files.join(", ")
                    );
                    1
                }
                CombineState::CheckFailed => {
                    let step = record.failing_step.as_deref().unwrap_or("unknown");
                    let _ = writeln!(err, "combine check failed at step: {step}");
                    1
                }
            }
        }
        Err(msg) => {
            let _ = writeln!(err, "error: {msg}");
            2
        }
    }
}

fn run_combine_inner(root: &Path, request: &str) -> Result<CombineRecord, String> {
    let wt_dir = root.join(".wt");
    let runs = discover_and_validate_runs(&wt_dir, request)?;
    let ordered = order_runs(runs)?;
    let run_id_set: BTreeSet<&str> = ordered.iter().map(|r| r.id.as_str()).collect();

    let group_base = resolve_group_base(root, &ordered, &run_id_set)?;

    let mut tips: HashMap<String, String> = HashMap::new();
    let mut combined_runs: Vec<CombinedRun> = Vec::with_capacity(ordered.len());
    for run in &ordered {
        let tip = resolve_run_tip(root, &wt_dir, run)?;
        tips.insert(run.id.clone(), tip.clone());
        combined_runs.push(CombinedRun {
            id: run.id.clone(),
            tip,
        });
    }

    let branch = format!("blyte/{request}");
    let worktree_dir = wt_dir.join(format!("combine-{request}"));
    let combined_dir = wt_dir.join("combined");
    let record_path = combined_dir.join(format!("{request}.json"));

    let branch_exists = git_branch_exists(root, &branch)?;
    let existing_record = load_record(&record_path)?;

    if branch_exists && existing_record.is_none() {
        return Err(format!(
            "branch {branch} already exists without a combine record at {}; refusing to overwrite a branch not created by blyte combine",
            record_path.display()
        ));
    }

    if let Some(ref rec) = existing_record {
        if rec.state == CombineState::Combined
            && branch_exists
            && rec.runs == combined_runs
            && rec.base == group_base
        {
            return Ok(rec.clone());
        }
    }

    let dispatch_cfg = load_gate_config(root)?;
    let gate = dispatch_cfg
        .gate
        .as_deref()
        .filter(|g| !gate_steps(g).is_empty())
        .ok_or_else(|| "blyte.toml has no [dispatch].gate configured".to_string())?
        .to_string();

    ensure_wt_excluded(root);
    std::fs::create_dir_all(&combined_dir)
        .map_err(|e| format!("failed to create {}: {e}", combined_dir.display()))?;

    prepare_worktree(root, &worktree_dir, &branch, &group_base)?;

    let mut last_good_head = group_base.clone();
    for run in &ordered {
        let tip = &tips[&run.id];
        let parent_base = match parent_run_id(run, &run_id_set) {
            Some(parent_id) => tips[parent_id].as_str(),
            None => group_base.as_str(),
        };
        let commits = git_rev_list_range(root, parent_base, tip)?;
        if commits.is_empty() {
            continue;
        }
        for sha in &commits {
            if let Err(conflict_files) = git_cherry_pick_x(&worktree_dir, sha)? {
                let _ = git_cmd(&worktree_dir, &["cherry-pick", "--abort"]);
                git_reset_hard(&worktree_dir, &last_good_head)?;
                let applied_commits = git_rev_list_range(&worktree_dir, &group_base, "HEAD")?;
                let record = CombineRecord {
                    request: request.to_string(),
                    state: CombineState::Conflict,
                    branch,
                    base: group_base,
                    runs: combined_runs,
                    commits: applied_commits,
                    conflicting_run: Some(run.id.clone()),
                    conflicting_files: conflict_files,
                    failing_step: None,
                    output_tail: None,
                };
                write_record(&record_path, &record)?;
                return Ok(record);
            }
        }
        last_good_head = git_rev_parse(&worktree_dir, "HEAD")?;
    }

    let combined_commits = git_rev_list_range(&worktree_dir, &group_base, "HEAD")?;

    if let Some(ref setup_cmd) = dispatch_cfg.setup {
        let main_checkout =
            crate::config::main_checkout(root).unwrap_or_else(|| root.to_path_buf());
        let setup_log_path = wt_dir.join(format!("combine-{request}.setup.log"));
        if let Err(msg) = crate::setup::run_setup(
            setup_cmd,
            &worktree_dir,
            &main_checkout,
            &dispatch_cfg.env,
            &setup_log_path,
            crate::setup::SETUP_TIMEOUT,
        ) {
            let record = CombineRecord {
                request: request.to_string(),
                state: CombineState::CheckFailed,
                branch,
                base: group_base,
                runs: combined_runs,
                commits: combined_commits,
                conflicting_run: None,
                conflicting_files: Vec::new(),
                failing_step: Some("setup".to_string()),
                output_tail: Some(tail_output(&msg, 50, 4000)),
            };
            write_record(&record_path, &record)?;
            return Ok(record);
        }
    }

    let shell = EnvShell {
        inner: LocalShell::new(),
        env: dispatch_cfg.env.clone(),
    };
    let mut deadline = None;
    let gate_result = run_gate_steps(
        &shell,
        &gate,
        &worktree_dir,
        &[],
        &mut deadline,
        |_| {},
        &std::time::Instant::now,
    );

    let record = if gate_result.passed {
        CombineRecord {
            request: request.to_string(),
            state: CombineState::Combined,
            branch,
            base: group_base,
            runs: combined_runs,
            commits: combined_commits,
            conflicting_run: None,
            conflicting_files: Vec::new(),
            failing_step: None,
            output_tail: None,
        }
    } else {
        let (failing_step, raw_output) = match gate_result.failures.first() {
            Some(f) => (f.step.clone(), f.output.as_str()),
            None => (gate.clone(), gate_result.output.as_str()),
        };
        CombineRecord {
            request: request.to_string(),
            state: CombineState::CheckFailed,
            branch,
            base: group_base,
            runs: combined_runs,
            commits: combined_commits,
            conflicting_run: None,
            conflicting_files: Vec::new(),
            failing_step: Some(failing_step),
            output_tail: Some(tail_output(raw_output, 50, 4000)),
        }
    };

    write_record(&record_path, &record)?;
    Ok(record)
}

fn discover_and_validate_runs(wt_dir: &Path, request: &str) -> Result<Vec<RunMeta>, String> {
    if !wt_dir.is_dir() {
        return Err(format!("no runs found for request {request}"));
    }

    let mut entries: Vec<PathBuf> = std::fs::read_dir(wt_dir)
        .map_err(|e| format!("failed to read {}: {e}", wt_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".start.json"))
        })
        .collect();
    entries.sort();

    let mut found = Vec::new();
    let mut bad_runs = Vec::new();

    for path in entries {
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let val: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("request").and_then(|v| v.as_str()) != Some(request) {
            continue;
        }
        let is_research = val
            .get("research")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || val
                .get("tags")
                .and_then(|v| v.as_array())
                .is_some_and(|arr| arr.iter().any(|t| t.as_str() == Some("research")));
        if is_research {
            continue;
        }
        let fallback_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".start.json"))
            .unwrap_or("")
            .to_string();
        let id = val
            .get("id")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .unwrap_or(fallback_id);
        let started_at = val.get("started_at").and_then(|v| v.as_u64()).unwrap_or(0);
        let base = val
            .get("base")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let base_sha = val
            .get("base_sha")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        let summary_path = wt_dir.join(format!("{id}.json"));
        let summary: Option<serde_json::Value> = std::fs::read_to_string(&summary_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());

        let mut reasons = Vec::new();
        let mut branch_hint = None;
        match summary {
            None => {
                reasons.push("unfinished".to_string());
            }
            Some(ref s) => {
                let outcome = s
                    .get("outcome")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                if outcome != "verified" {
                    reasons.push(outcome.to_string());
                }
                branch_hint = s
                    .get("branch")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
            }
        }

        let verdict_path = wt_dir.join(format!("{id}.verdict.json"));
        if let Ok(vs) = std::fs::read_to_string(&verdict_path) {
            if let Ok(vv) = serde_json::from_str::<serde_json::Value>(&vs) {
                if vv.get("verdict").and_then(|v| v.as_str()) == Some("redone") {
                    reasons.push("verdict: redone".to_string());
                }
            }
        }

        if !reasons.is_empty() {
            bad_runs.push((started_at, id, reasons.join(", ")));
        } else {
            found.push(RunMeta {
                id,
                started_at,
                base,
                base_sha,
                branch_hint,
            });
        }
    }

    if !bad_runs.is_empty() {
        bad_runs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let details: Vec<String> = bad_runs
            .into_iter()
            .map(|(_, id, state)| format!("{id} ({state})"))
            .collect();
        return Err(format!(
            "cannot combine request {request}; runs not ready: {}",
            details.join(", ")
        ));
    }

    if found.is_empty() {
        return Err(format!("no runs found for request {request}"));
    }

    Ok(found)
}

fn parent_run_id<'a>(run: &'a RunMeta, run_ids: &BTreeSet<&str>) -> Option<&'a str> {
    let base = run.base.as_deref()?;
    let other = base.strip_prefix("wip/")?;
    if run_ids.contains(other) {
        Some(other)
    } else {
        None
    }
}

fn order_runs(runs: Vec<RunMeta>) -> Result<Vec<RunMeta>, String> {
    let by_id: HashMap<String, RunMeta> = runs.into_iter().map(|r| (r.id.clone(), r)).collect();
    let id_set: BTreeSet<&str> = by_id.keys().map(String::as_str).collect();

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for (id, run) in &by_id {
        if let Some(parent) = parent_run_id(run, &id_set) {
            in_degree.insert(id.clone(), 1);
            children
                .entry(parent.to_string())
                .or_default()
                .push(id.clone());
        } else {
            in_degree.insert(id.clone(), 0);
        }
    }

    let mut ready: BTreeSet<(u64, String)> = BTreeSet::new();
    for (id, &deg) in &in_degree {
        if deg == 0 {
            ready.insert((by_id[id].started_at, id.clone()));
        }
    }

    let mut ordered = Vec::with_capacity(by_id.len());
    while let Some((_, id)) = ready.pop_first() {
        ordered.push(by_id[&id].clone());
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                let deg = in_degree.get_mut(kid).expect("kid in in_degree");
                *deg -= 1;
                if *deg == 0 {
                    ready.insert((by_id[kid].started_at, kid.clone()));
                }
            }
        }
    }

    if ordered.len() != by_id.len() {
        let mut remaining: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg > 0)
            .map(|(id, _)| id.as_str())
            .collect();
        remaining.sort();
        return Err(format!(
            "cyclic base dependency among runs: {}",
            remaining.join(", ")
        ));
    }

    Ok(ordered)
}

fn resolve_group_base(
    root: &Path,
    ordered: &[RunMeta],
    run_ids: &BTreeSet<&str>,
) -> Result<String, String> {
    let mut root_bases: Vec<(String, String)> = Vec::new();
    for run in ordered {
        if parent_run_id(run, run_ids).is_some() {
            continue;
        }
        let spec = run
            .base_sha
            .as_deref()
            .or(run.base.as_deref())
            .ok_or_else(|| format!("run {} has no base or base_sha in start.json", run.id))?;
        let sha = git_rev_parse(root, &format!("{spec}^{{commit}}"))
            .map_err(|e| format!("cannot resolve base {spec} for run {}: {e}", run.id))?;
        root_bases.push((run.id.clone(), sha));
    }

    if root_bases.is_empty() {
        return Err("no root run found to determine base commit".to_string());
    }

    let first_sha = &root_bases[0].1;
    let all_same = root_bases.iter().all(|(_, sha)| sha == first_sha);
    if !all_same {
        let details: Vec<String> = root_bases
            .iter()
            .map(|(id, sha)| format!("{id} ({})", &sha[..sha.len().min(7)]))
            .collect();
        return Err(format!(
            "runs started from different base commits: {}",
            details.join(", ")
        ));
    }

    Ok(first_sha.clone())
}

fn resolve_run_tip(root: &Path, wt_dir: &Path, run: &RunMeta) -> Result<String, String> {
    let mut candidates = vec![format!("refs/heads/wip/{}", run.id)];
    if let Some(ref b) = run.branch_hint {
        let full = format!("refs/heads/{b}");
        if !candidates.contains(&full) {
            candidates.push(full);
        }
    }
    for cand in &candidates {
        if let Ok(sha) = git_rev_parse(root, &format!("{cand}^{{commit}}")) {
            return Ok(sha);
        }
    }
    let run_wt = wt_dir.join(&run.id);
    if run_wt.is_dir() {
        if let Ok(sha) = git_rev_parse(&run_wt, "HEAD^{commit}") {
            return Ok(sha);
        }
    }
    Err(format!(
        "cannot resolve tip commit for run {} (expected branch wip/{})",
        run.id, run.id
    ))
}

fn load_gate_config(root: &Path) -> Result<crate::config::DispatchConfig, String> {
    match crate::config::load_with_fallback(root)? {
        Some((cfg, _)) => Ok(cfg.dispatch),
        None => Err("no blyte.toml found in repository".to_string()),
    }
}

fn load_record(path: &Path) -> Result<Option<CombineRecord>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let record: CombineRecord = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    Ok(Some(record))
}

fn write_record(path: &Path, record: &CombineRecord) -> Result<(), String> {
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("failed to serialize combine record: {e}"))?;
    std::fs::write(path, json + "\n")
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn prepare_worktree(
    root: &Path,
    worktree_dir: &Path,
    branch: &str,
    base_commit: &str,
) -> Result<(), String> {
    if worktree_dir.exists() {
        let _ = git_cmd(worktree_dir, &["cherry-pick", "--abort"]);
        let _ = git_cmd(
            root,
            &[
                "worktree",
                "remove",
                "--force",
                worktree_dir.to_str().unwrap_or_default(),
            ],
        );
        if worktree_dir.exists() {
            std::fs::remove_dir_all(worktree_dir).map_err(|e| {
                format!(
                    "failed to clean existing worktree {}: {e}",
                    worktree_dir.display()
                )
            })?;
        }
    }
    let _ = git_cmd(root, &["worktree", "prune"]);

    if git_branch_exists(root, branch)? {
        git_cmd(root, &["branch", "-D", branch])
            .map_err(|e| format!("failed to reset branch {branch}: {e}"))?;
    }

    let wt_str = worktree_dir
        .to_str()
        .ok_or_else(|| format!("non-UTF-8 worktree path: {}", worktree_dir.display()))?;
    git_cmd(
        root,
        &["worktree", "add", "-b", branch, wt_str, base_commit],
    )
    .map_err(|e| format!("failed to create combine worktree {wt_str}: {e}"))?;
    Ok(())
}

fn ensure_wt_excluded(root: &Path) {
    if let Ok(common) = git_cmd(root, &["rev-parse", "--git-common-dir"]) {
        let common_path = PathBuf::from(common.trim());
        let common_dir = if common_path.is_absolute() {
            common_path
        } else {
            root.join(common_path)
        };
        let exclude_path = common_dir.join("info").join("exclude");
        if let Some(parent) = exclude_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = std::fs::read_to_string(&exclude_path).unwrap_or_default();
        if !content.lines().any(|l| l.trim() == ".wt/") {
            let mut next = content;
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(".wt/\n");
            let _ = std::fs::write(&exclude_path, next);
        }
    }
}

fn git_cmd(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to execute git {}: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        Err(detail)
    }
}

fn git_rev_parse(dir: &Path, rev: &str) -> Result<String, String> {
    Ok(git_cmd(dir, &["rev-parse", "--verify", rev])?
        .trim()
        .to_string())
}

fn git_branch_exists(dir: &Path, branch: &str) -> Result<bool, String> {
    let out = Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to check branch {branch}: {e}"))?;
    Ok(out.status.success())
}

fn git_rev_list_range(dir: &Path, base: &str, tip: &str) -> Result<Vec<String>, String> {
    let range = format!("{base}..{tip}");
    let out = git_cmd(dir, &["rev-list", "--reverse", &range])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn git_cherry_pick_x(worktree_dir: &Path, sha: &str) -> Result<Result<(), Vec<String>>, String> {
    let out = Command::new("git")
        .args(["cherry-pick", "-x", sha])
        .current_dir(worktree_dir)
        .output()
        .map_err(|e| format!("failed to execute git cherry-pick -x {sha}: {e}"))?;
    if out.status.success() {
        return Ok(Ok(()));
    }
    let unmerged = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .current_dir(worktree_dir)
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Err(unmerged))
}

fn git_reset_hard(worktree_dir: &Path, rev: &str) -> Result<(), String> {
    git_cmd(worktree_dir, &["reset", "--hard", rev])?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed:\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo(gate: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Test User"]);
        git(root, &["config", "user.email", "test@example.com"]);
        std::fs::write(
            root.join("blyte.toml"),
            format!("[dispatch]\ngate = \"\"\"\n{gate}\n\"\"\"\n"),
        )
        .unwrap();
        std::fs::write(root.join("shared.txt"), "base line\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "--no-gpg-sign", "-m", "initial"]);
        let base_sha = git(root, &["rev-parse", "HEAD"]);
        (dir, base_sha)
    }

    fn create_run_branch(
        root: &Path,
        id: &str,
        request: &str,
        base_ref: &str,
        started_at: u64,
        commits: &[(&str, &str, &str)],
        outcome: Option<&str>,
    ) -> String {
        let wt_dir = root.join(".wt");
        std::fs::create_dir_all(&wt_dir).unwrap();
        let run_wt = wt_dir.join(id);
        let branch = format!("wip/{id}");
        let base_sha = git(root, &["rev-parse", &format!("{base_ref}^{{commit}}")]);
        git(
            root,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                run_wt.to_str().unwrap(),
                &base_sha,
            ],
        );
        for (file, content, msg) in commits {
            let file_path = run_wt.join(file);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&file_path, content).unwrap();
            git(&run_wt, &["add", file]);
            git(&run_wt, &["commit", "--no-gpg-sign", "-m", msg]);
        }
        let tip = git(&run_wt, &["rev-parse", "HEAD"]);
        let start_json = serde_json::json!({
            "id": id,
            "request": request,
            "base": base_ref,
            "base_sha": base_sha,
            "started_at": started_at,
            "research": false,
        });
        std::fs::write(
            wt_dir.join(format!("{id}.start.json")),
            serde_json::to_string_pretty(&start_json).unwrap(),
        )
        .unwrap();
        if let Some(out) = outcome {
            let summary_json = serde_json::json!({
                "id": id,
                "branch": branch,
                "outcome": out,
            });
            std::fs::write(
                wt_dir.join(format!("{id}.json")),
                serde_json::to_string_pretty(&summary_json).unwrap(),
            )
            .unwrap();
        }
        tip
    }

    #[test]
    fn cli_parses_combine() {
        use crate::cli::{Cli, Command};
        use clap::Parser;

        let cli = Cli::try_parse_from(["blyte", "combine", "req-42"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Combine(CombineArgs {
                request: "req-42".to_string(),
            }))
        );
        assert!(Cli::try_parse_from(["blyte", "combine"]).is_err());
    }

    #[test]
    fn two_independent_runs_combine_and_gate_runs() {
        let (dir, base_sha) = init_repo("test -f a.txt && test -f b.txt");
        let root = dir.path();
        let tip_a = create_run_branch(
            root,
            "T1",
            "req1",
            "main",
            100,
            &[("a.txt", "alpha\n", "add a")],
            Some("verified"),
        );
        let tip_b = create_run_branch(
            root,
            "T2",
            "req1",
            "main",
            200,
            &[("b.txt", "beta\n", "add b")],
            Some("verified"),
        );

        let research_start = serde_json::json!({
            "id": "R1",
            "request": "req1",
            "base": "main",
            "base_sha": base_sha,
            "started_at": 50,
            "research": true,
        });
        std::fs::write(
            root.join(".wt/R1.start.json"),
            serde_json::to_string_pretty(&research_start).unwrap(),
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req1", &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::Combined);
        assert_eq!(record.branch, "blyte/req1");
        assert_eq!(record.base, base_sha);
        assert_eq!(
            record.runs,
            vec![
                CombinedRun {
                    id: "T1".to_string(),
                    tip: tip_a.clone(),
                },
                CombinedRun {
                    id: "T2".to_string(),
                    tip: tip_b.clone(),
                },
            ]
        );
        assert_eq!(record.commits.len(), 2);

        let combine_wt = root.join(".wt/combine-req1");
        assert!(combine_wt.join("a.txt").exists());
        assert!(combine_wt.join("b.txt").exists());
        assert!(!root.join("a.txt").exists());
        assert!(!root.join("b.txt").exists());

        let log_msg = git(&combine_wt, &["log", "-n", "1", "--format=%B"]);
        assert!(log_msg.contains(&format!("(cherry picked from commit {tip_b})")));
    }

    #[test]
    fn dependent_run_applies_only_its_own_commits() {
        let (dir, _base_sha) = init_repo("test -f p.txt && test -f c.txt");
        let root = dir.path();
        let tip_parent = create_run_branch(
            root,
            "T1",
            "req-dep",
            "main",
            200,
            &[("p.txt", "parent\n", "add parent")],
            Some("verified"),
        );
        let tip_child = create_run_branch(
            root,
            "T2",
            "req-dep",
            "wip/T1",
            100,
            &[("c.txt", "child\n", "add child")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-dep", &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-dep.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::Combined);
        assert_eq!(
            record.runs,
            vec![
                CombinedRun {
                    id: "T1".to_string(),
                    tip: tip_parent,
                },
                CombinedRun {
                    id: "T2".to_string(),
                    tip: tip_child,
                },
            ]
        );
        assert_eq!(record.commits.len(), 2);

        let combine_wt = root.join(".wt/combine-req-dep");
        let subjects = git(
            &combine_wt,
            &["log", "--reverse", "--format=%s", "main..HEAD"],
        );
        assert_eq!(subjects, "add parent\nadd child");
    }

    #[test]
    fn conflict_aborts_and_names_run_and_files() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-conflict",
            "main",
            100,
            &[("shared.txt", "change from T1\n", "T1 edits shared")],
            Some("verified"),
        );
        create_run_branch(
            root,
            "T2",
            "req-conflict",
            "main",
            200,
            &[
                ("extra.txt", "clean first commit\n", "T2 clean commit"),
                ("shared.txt", "change from T2\n", "T2 conflicting edit"),
            ],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-conflict", &mut out, &mut err);
        assert_ne!(code, 0);

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-conflict.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::Conflict);
        assert_eq!(record.conflicting_run.as_deref(), Some("T2"));
        assert_eq!(record.conflicting_files, vec!["shared.txt".to_string()]);
        assert_eq!(record.commits.len(), 1);

        let combine_wt = root.join(".wt/combine-req-conflict");
        assert!(!combine_wt.join("extra.txt").exists());
        assert_eq!(
            std::fs::read_to_string(combine_wt.join("shared.txt")).unwrap(),
            "change from T1\n"
        );
        let status = git(&combine_wt, &["status", "--porcelain"]);
        assert!(status.is_empty(), "worktree should be clean: {status}");
    }

    #[test]
    fn failing_gate_records_check_failed_with_step_and_output() {
        let (dir, _base_sha) = init_repo("true\necho broken-check >&2 && exit 1");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-fail",
            "main",
            100,
            &[("a.txt", "hello\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-fail", &mut out, &mut err);
        assert_ne!(code, 0);

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-fail.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::CheckFailed);
        assert_eq!(
            record.failing_step.as_deref(),
            Some("echo broken-check >&2 && exit 1")
        );
        assert!(record
            .output_tail
            .as_deref()
            .unwrap_or("")
            .contains("broken-check"));
    }

    #[test]
    fn unfinished_unverified_and_redone_runs_are_refused_by_name() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-bad",
            "main",
            100,
            &[("a.txt", "a\n", "a")],
            None,
        );
        create_run_branch(
            root,
            "T2",
            "req-bad",
            "main",
            200,
            &[("b.txt", "b\n", "b")],
            Some("gate_failed"),
        );
        create_run_branch(
            root,
            "T3",
            "req-bad",
            "main",
            300,
            &[("c.txt", "c\n", "c")],
            Some("verified"),
        );
        std::fs::write(
            root.join(".wt/T3.verdict.json"),
            r#"{"verdict":"redone","notes":"redo","ts":1}"#,
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-bad", &mut out, &mut err);
        assert_ne!(code, 0);

        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T1") && err_str.contains("unfinished"),
            "{err_str}"
        );
        assert!(
            err_str.contains("T2") && err_str.contains("gate_failed"),
            "{err_str}"
        );
        assert!(
            err_str.contains("T3") && err_str.contains("redone"),
            "{err_str}"
        );
    }

    #[test]
    fn second_call_with_same_tips_is_noop_and_different_tips_rebuilds() {
        let marker = tempfile::NamedTempFile::new().unwrap();
        let marker_path = marker.path().to_path_buf();
        let gate = format!("echo run >> '{}'", marker_path.display());
        let (dir, _base_sha) = init_repo(&gate);
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-idem",
            "main",
            100,
            &[("a.txt", "v1\n", "v1")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(combine_in(root, "req-idem", &mut out, &mut err), 0);
        let first_head = git(root, &["rev-parse", "blyte/req-idem"]);
        let runs_after_first = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();
        assert_eq!(runs_after_first, 1);

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-idem", &mut out, &mut err), 0);
        let second_head = git(root, &["rev-parse", "blyte/req-idem"]);
        let runs_after_second = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();
        assert_eq!(first_head, second_head);
        assert_eq!(runs_after_second, 1);

        let t1_wt = root.join(".wt/T1");
        std::fs::write(t1_wt.join("a.txt"), "v2\n").unwrap();
        git(&t1_wt, &["add", "a.txt"]);
        git(&t1_wt, &["commit", "--no-gpg-sign", "-m", "v2"]);

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-idem", &mut out, &mut err), 0);
        let third_head = git(root, &["rev-parse", "blyte/req-idem"]);
        let runs_after_third = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();
        assert_ne!(third_head, first_head);
        assert_eq!(runs_after_third, 2);
        assert_eq!(
            std::fs::read_to_string(root.join(".wt/combine-req-idem/a.txt")).unwrap(),
            "v2\n"
        );
    }

    #[test]
    fn existing_foreign_branch_is_refused() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-foreign",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        git(root, &["branch", "blyte/req-foreign", "main"]);
        let before = git(root, &["rev-parse", "blyte/req-foreign"]);

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-foreign", &mut out, &mut err);
        assert_ne!(code, 0);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("blyte/req-foreign"), "{err_str}");
        assert_eq!(git(root, &["rev-parse", "blyte/req-foreign"]), before);
        assert!(!root.join(".wt/combined/req-foreign.json").exists());
    }

    #[test]
    fn mismatched_root_bases_and_no_runs_are_errors() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "missing-req", &mut out, &mut err);
        assert_ne!(code, 0);
        assert!(String::from_utf8_lossy(&err).contains("missing-req"));

        create_run_branch(
            root,
            "T1",
            "req-diffbase",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        std::fs::write(root.join("main_extra.txt"), "extra\n").unwrap();
        git(root, &["add", "main_extra.txt"]);
        git(root, &["commit", "--no-gpg-sign", "-m", "advance main"]);
        create_run_branch(
            root,
            "T2",
            "req-diffbase",
            "main",
            200,
            &[("b.txt", "b\n", "add b")],
            Some("verified"),
        );

        out.clear();
        err.clear();
        let code = combine_in(root, "req-diffbase", &mut out, &mut err);
        assert_ne!(code, 0);
        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T1") && err_str.contains("T2"),
            "{err_str}"
        );
    }

    #[test]
    fn dispatch_env_is_passed_to_gate() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        std::fs::write(
            root.join("blyte.toml"),
            "[dispatch]\ngate = \"test \\\"$MY_COMBINE_FLAG\\\" = \\\"enabled\\\"\"\n\n[dispatch.env]\nMY_COMBINE_FLAG = \"enabled\"\n",
        )
        .unwrap();

        create_run_branch(
            root,
            "T1",
            "req-env",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-env", &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
    }

    #[test]
    fn rebuilds_from_start_after_failure() {
        let (dir, _base_sha) = init_repo("test -f ready.txt");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-resume",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_ne!(combine_in(root, "req-resume", &mut out, &mut err), 0);
        let first_record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-resume.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(first_record.state, CombineState::CheckFailed);

        std::fs::write(
            root.join("blyte.toml"),
            "[dispatch]\ngate = \"test -f a.txt\"\n",
        )
        .unwrap();

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-resume", &mut out, &mut err), 0);
        let second_record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-resume.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(second_record.state, CombineState::Combined);
    }

    #[test]
    fn research_run_with_tag_is_ignored() {
        let (dir, base_sha) = init_repo("test -f a.txt");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-res-tag",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        let research_start = serde_json::json!({
            "id": "R2",
            "request": "req-res-tag",
            "base": "main",
            "base_sha": base_sha,
            "started_at": 50,
            "tags": ["research"],
        });
        std::fs::write(
            root.join(".wt/R2.start.json"),
            serde_json::to_string_pretty(&research_start).unwrap(),
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-res-tag", &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-res-tag.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.runs.len(), 1);
        assert_eq!(record.runs[0].id, "T1");
    }
}
