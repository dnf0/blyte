//! `blyte combine <request>`: combine a request's passed runs on a work branch and run the gate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use blyte_agent::run::{cut_gate_output, gate_steps, run_gate_steps};
use blyte_agent::shell::{LocalShell, Output, Shell};
use serde::{Deserialize, Serialize};

use crate::cli::{CombineArgs, ReviewVerdict};

pub(crate) fn is_left_out_verdict(verdict: Option<ReviewVerdict>) -> bool {
    verdict == Some(ReviewVerdict::Redone)
}

pub(crate) fn is_run_left_out(root: &Path, id: &str) -> Result<bool, String> {
    Ok(is_left_out_verdict(crate::queue_actions::read_verdict(
        root, id,
    )?))
}

pub(crate) fn base_run_id(base: Option<&str>) -> Option<&str> {
    let base = base?;
    let clean = base.strip_prefix("refs/heads/").unwrap_or(base);
    clean.strip_prefix("wip/")
}

pub(crate) fn check_not_based_on_left_out(
    root: &Path,
    run_id: &str,
    base: Option<&str>,
    left_out_ids: &BTreeSet<&str>,
) -> Result<(), String> {
    let Some(parent_id) = base_run_id(base) else {
        return Ok(());
    };
    let parent_left_out = left_out_ids.contains(parent_id) || is_run_left_out(root, parent_id)?;
    if parent_left_out {
        return Err(format!(
            "run {run_id} was dispatched from left-out run {parent_id} (branch wip/{parent_id})"
        ));
    }
    Ok(())
}

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

/// Whether the combine check actually ran or was skipped because the tree had already passed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineCheck {
    #[default]
    Ran,
    AlreadyPassed,
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
    #[serde(default)]
    pub check: CombineCheck,
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

struct RepoDirs {
    common_dir: Option<PathBuf>,
    main_checkout: PathBuf,
}

fn main_checkout_from_common_dir(root: &Path, common_dir: &Path) -> Option<PathBuf> {
    if common_dir.file_name().and_then(|n| n.to_str()) != Some(".git") {
        return None;
    }
    let parent = common_dir.parent()?;
    let parent_canon = parent.canonicalize().ok()?;
    let root_canon = root.canonicalize().ok()?;
    if parent_canon == root_canon {
        return None;
    }
    Some(parent.to_path_buf())
}

fn find_combine_repo() -> Result<(PathBuf, RepoDirs), String> {
    let out = Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ])
        .output()
        .map_err(|e| format!("git not found: {e}"))?;
    if !out.status.success() {
        return Err("not a git repository".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let (Some(toplevel), Some(git_dir), Some(common_dir)) =
        (lines.next(), lines.next(), lines.next())
    else {
        return Err("cannot read git repository paths".to_string());
    };
    let toplevel_path = PathBuf::from(toplevel.trim());
    let common_path = PathBuf::from(common_dir.trim());
    let root = if git_dir.trim() != common_dir.trim() {
        main_checkout_from_common_dir(&toplevel_path, &common_path).ok_or_else(|| {
            format!(
                "cannot find main checkout for linked worktree {}",
                toplevel_path.display()
            )
        })?
    } else {
        toplevel_path
    };
    let main_checkout = root.clone();
    Ok((
        root,
        RepoDirs {
            common_dir: Some(common_path),
            main_checkout,
        },
    ))
}

fn resolve_repo_dirs(root: &Path) -> RepoDirs {
    let common_dir = git_cmd(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .map(PathBuf::from);
    let main_checkout = common_dir
        .as_deref()
        .and_then(|cd| main_checkout_from_common_dir(root, cd))
        .unwrap_or_else(|| root.to_path_buf());
    RepoDirs {
        common_dir,
        main_checkout,
    }
}

/// Entry point for `blyte combine <request>`.
pub fn run(args: &CombineArgs) -> i32 {
    if let Err(msg) = crate::dispatch::validate_request_id(&args.request) {
        eprintln!("error: {msg}");
        return 2;
    }
    let (root, dirs) = match find_combine_repo() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    combine_in_with_dirs(&root, Some(dirs), &args.request, &mut stdout, &mut stderr)
}

/// Combine all passed non-research runs for `request` inside repository `root`.
pub fn combine_in(root: &Path, request: &str, out: &mut impl Write, err: &mut impl Write) -> i32 {
    combine_in_with_dirs(root, None, request, out, err)
}

fn combine_in_with_dirs(
    root: &Path,
    dirs: Option<RepoDirs>,
    request: &str,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    if let Err(msg) = crate::dispatch::validate_request_id(request) {
        let _ = writeln!(err, "error: {msg}");
        return 2;
    }

    match run_combine_inner(root, dirs, request) {
        Ok(record) => {
            let json = match serde_json::to_string_pretty(&record) {
                Ok(j) => j,
                Err(e) => {
                    let _ = writeln!(err, "error: failed to serialize combine record: {e}");
                    return 2;
                }
            };
            let _ = writeln!(out, "{json}");
            match record.state {
                CombineState::Combined => {
                    let check_label = match record.check {
                        CombineCheck::Ran => "ran",
                        CombineCheck::AlreadyPassed => "already_passed",
                    };
                    let _ = writeln!(err, "combine check: {check_label}");
                    0
                }
                CombineState::Conflict => {
                    let Some(run_id) = record.conflicting_run.as_deref() else {
                        let _ = writeln!(err, "error: combine record is missing conflicting_run");
                        return 2;
                    };
                    let _ = writeln!(
                        err,
                        "combine conflict while applying run {run_id}: {}",
                        record.conflicting_files.join(", ")
                    );
                    1
                }
                CombineState::CheckFailed => {
                    let Some(step) = record.failing_step.as_deref() else {
                        let _ = writeln!(err, "error: combine record is missing failing_step");
                        return 2;
                    };
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

fn run_combine_inner(
    root: &Path,
    dirs: Option<RepoDirs>,
    request: &str,
) -> Result<CombineRecord, String> {
    let wt_dir = root.join(".wt");
    let runs = discover_and_validate_runs(root, request)?;
    let ordered = order_runs(runs)?;
    let run_id_set: BTreeSet<&str> = ordered.iter().map(|r| r.id.as_str()).collect();

    let branch = format!("blyte/{request}");
    let worktree_dir = wt_dir.join(format!("combine-{request}"));
    let combined_dir = wt_dir.join("combined");
    let record_path = combined_dir.join(format!("{request}.json"));

    let repo_dirs = dirs.unwrap_or_else(|| resolve_repo_dirs(root));
    let starting = resolve_starting_refs(root, &ordered, &run_id_set, &branch)?;
    let group_base = starting.group_base;
    let tips = starting.tips;
    let combined_runs = starting.combined_runs;
    let existing_branch_tip = starting.existing_branch_tip;

    let existing_record = load_record(&record_path)?;

    if existing_branch_tip.is_some() && existing_record.is_none() {
        return Err(format!(
            "branch {branch} already exists without a combine record at {}; refusing to overwrite a branch not created by blyte combine",
            record_path.display()
        ));
    }

    let expected_existing_tip = existing_record
        .as_ref()
        .map(|rec| rec.commits.last().unwrap_or(&rec.base).as_str());

    if let (Some(ref rec), Some(ref actual_tip)) = (&existing_record, &existing_branch_tip) {
        if rec.state == CombineState::Combined
            && rec.runs == combined_runs
            && rec.base == group_base
        {
            let expected_tip = rec.commits.last().unwrap_or(&rec.base).as_str();
            if actual_tip == expected_tip {
                return Ok(rec.clone());
            }
        }
    }

    let dispatch_cfg = load_gate_config(root, &repo_dirs.main_checkout)?;
    let gate = dispatch_cfg
        .gate
        .as_deref()
        .filter(|g| !gate_steps(g).is_empty())
        .ok_or_else(|| "blyte.toml has no [dispatch].gate configured".to_string())?
        .to_string();

    ensure_wt_excluded(repo_dirs.common_dir.as_deref());
    std::fs::create_dir_all(&combined_dir)
        .map_err(|e| format!("failed to create {}: {e}", combined_dir.display()))?;

    prepare_worktree(
        root,
        &worktree_dir,
        &branch,
        &group_base,
        existing_branch_tip.as_deref(),
        expected_existing_tip,
    )?;

    let mut last_good_head = group_base.clone();
    for (idx, run) in ordered.iter().enumerate() {
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
            match git_cherry_pick_x(&worktree_dir, &run.id, sha) {
                Ok(Ok(())) => {}
                Ok(Err(conflict_files)) => {
                    if has_cherry_pick_head(&worktree_dir) {
                        let _ = git_cmd(&worktree_dir, &["cherry-pick", "--abort"]);
                    }
                    git_reset_hard(&worktree_dir, &last_good_head)?;
                    let applied_commits = git_rev_list_range(&worktree_dir, &group_base, "HEAD")?;
                    let record = CombineRecord {
                        request: request.to_string(),
                        state: CombineState::Conflict,
                        branch,
                        base: group_base,
                        runs: combined_runs,
                        commits: applied_commits,
                        check: CombineCheck::Ran,
                        conflicting_run: Some(run.id.clone()),
                        conflicting_files: conflict_files,
                        failing_step: None,
                        output_tail: None,
                    };
                    write_record(&record_path, &record)?;
                    return Ok(record);
                }
                Err(err_msg) => {
                    if has_cherry_pick_head(&worktree_dir) {
                        let _ = git_cmd(&worktree_dir, &["cherry-pick", "--abort"]);
                    }
                    git_reset_hard(&worktree_dir, &last_good_head)?;
                    return Err(err_msg);
                }
            }
        }
        if idx + 1 < ordered.len() {
            last_good_head = git_rev_parse(&worktree_dir, "HEAD")?;
        }
    }

    let (combined_commits, last_tree) = git_log_commits_and_tree(&worktree_dir, &group_base)?;

    if let Some(ref setup_cmd) = dispatch_cfg.setup {
        let setup_log_path = wt_dir.join(format!("combine-{request}.setup.log"));
        if let Err(msg) = crate::setup::run_setup(
            setup_cmd,
            &worktree_dir,
            &repo_dirs.main_checkout,
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
                check: CombineCheck::Ran,
                conflicting_run: None,
                conflicting_files: Vec::new(),
                failing_step: Some("setup".to_string()),
                output_tail: Some(tail_output(&msg, 50, 4000)),
            };
            write_record(&record_path, &record)?;
            return Ok(record);
        }
    }

    let head_tree = match last_tree {
        Some(tree) => tree,
        None => crate::passed_trees::tree_of(&worktree_dir, "HEAD")?,
    };
    let key = crate::passed_trees::check_key(&gate, &dispatch_cfg.env);
    let clean_before = crate::passed_trees::worktree_is_clean(&worktree_dir)?;
    let already_passed = clean_before
        && crate::passed_trees::has_passed_in_main(
            &repo_dirs.main_checkout,
            &head_tree,
            &key,
            crate::passed_trees::now_secs(),
        )?;

    let record = if already_passed {
        CombineRecord {
            request: request.to_string(),
            state: CombineState::Combined,
            branch,
            base: group_base,
            runs: combined_runs,
            commits: combined_commits,
            check: CombineCheck::AlreadyPassed,
            conflicting_run: None,
            conflicting_files: Vec::new(),
            failing_step: None,
            output_tail: None,
        }
    } else {
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

        if gate_result.passed {
            if crate::passed_trees::worktree_is_clean(&worktree_dir)? {
                crate::passed_trees::record_passed_in_main(
                    &repo_dirs.main_checkout,
                    repo_dirs.common_dir.as_deref(),
                    &head_tree,
                    &key,
                )?;
            }
            CombineRecord {
                request: request.to_string(),
                state: CombineState::Combined,
                branch,
                base: group_base,
                runs: combined_runs,
                commits: combined_commits,
                check: CombineCheck::Ran,
                conflicting_run: None,
                conflicting_files: Vec::new(),
                failing_step: None,
                output_tail: None,
            }
        } else {
            let (failing_step, output_tail) = match gate_result.failures.as_slice() {
                [] => {
                    return Err(format!(
                        "combine check failed for {request} but no failing step was recorded"
                    ));
                }
                [f] => (f.step.clone(), cut_gate_output(&f.output)),
                many => {
                    let steps = many
                        .iter()
                        .map(|f| f.step.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    (steps, cut_gate_output(&gate_result.output))
                }
            };
            CombineRecord {
                request: request.to_string(),
                state: CombineState::CheckFailed,
                branch,
                base: group_base,
                runs: combined_runs,
                commits: combined_commits,
                check: CombineCheck::Ran,
                conflicting_run: None,
                conflicting_files: Vec::new(),
                failing_step: Some(failing_step),
                output_tail: Some(output_tail),
            }
        }
    };

    write_record(&record_path, &record)?;
    Ok(record)
}

fn discover_and_validate_runs(root: &Path, request: &str) -> Result<Vec<RunMeta>, String> {
    let wt_dir = root.join(".wt");
    if !wt_dir.is_dir() {
        return Err(format!("no runs found for request {request}"));
    }

    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&wt_dir)
        .map_err(|e| format!("failed to read {}: {e}", wt_dir.display()))?
    {
        let entry =
            entry.map_err(|e| format!("failed to read entry in {}: {e}", wt_dir.display()))?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".start.json"))
        {
            entries.push(path);
        }
    }
    entries.sort();

    let mut found = Vec::new();
    let mut bad_runs = Vec::new();
    let mut active_bases: Vec<(String, Option<String>)> = Vec::new();
    let mut left_out_ids: BTreeSet<String> = BTreeSet::new();

    for path in entries {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let val: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        if val.get("request").and_then(|v| v.as_str()) != Some(request) {
            continue;
        }
        let file_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".start.json"))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("invalid start file name: {}", path.display()))?;
        if let Some(id_val) = val.get("id") {
            let field_id = id_val.as_str().ok_or_else(|| {
                format!(
                    "start file {} has non-string id {id_val}, expected {file_id}",
                    path.display()
                )
            })?;
            if field_id != file_id {
                return Err(format!(
                    "start file {} has id {field_id} which differs from file name id {file_id}",
                    path.display()
                ));
            }
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
        let id = file_id.to_string();
        if is_run_left_out(root, &id)? {
            left_out_ids.insert(id);
            continue;
        }
        let started_at = val
            .get("started_at")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                format!(
                    "start file {} is missing numeric started_at",
                    path.display()
                )
            })?;
        let base = val
            .get("base")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let base_sha = val
            .get("base_sha")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        active_bases.push((id.clone(), base.clone()));

        let summary_path = wt_dir.join(format!("{id}.json"));
        let summary: Option<serde_json::Value> = match std::fs::read_to_string(&summary_path) {
            Ok(s) => {
                let parsed: serde_json::Value = serde_json::from_str(&s)
                    .map_err(|e| format!("failed to parse {}: {e}", summary_path.display()))?;
                Some(parsed)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(format!("failed to read {}: {e}", summary_path.display()));
            }
        };

        let mut reasons = Vec::new();
        let mut branch_hint = None;
        match summary {
            None => {
                reasons.push("unfinished".to_string());
            }
            Some(ref s) => {
                let outcome = s.get("outcome").and_then(|v| v.as_str()).ok_or_else(|| {
                    format!(
                        "summary file {} is missing string outcome",
                        summary_path.display()
                    )
                })?;
                if outcome != "verified" {
                    reasons.push(outcome.to_string());
                }
                if let Some(branch_val) = s.get("branch").filter(|v| !v.is_null()) {
                    let branch_str = branch_val.as_str().ok_or_else(|| {
                        format!(
                            "summary file {} has non-string branch",
                            summary_path.display()
                        )
                    })?;
                    branch_hint = Some(branch_str.to_string());
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

    let left_out_refs: BTreeSet<&str> = left_out_ids.iter().map(String::as_str).collect();
    for (id, base) in &active_bases {
        check_not_based_on_left_out(root, id, base.as_deref(), &left_out_refs)?;
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
        if !left_out_ids.is_empty() {
            return Err(format!(
                "cannot combine request {request}: every run was redone"
            ));
        }
        return Err(format!("no runs found for request {request}"));
    }

    Ok(found)
}

fn parent_run_id<'a>(run: &'a RunMeta, run_ids: &BTreeSet<&str>) -> Option<&'a str> {
    let other = base_run_id(run.base.as_deref())?;
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

struct StartingRefs {
    group_base: String,
    tips: HashMap<String, String>,
    combined_runs: Vec<CombinedRun>,
    existing_branch_tip: Option<String>,
}

fn resolve_starting_refs(
    root: &Path,
    ordered: &[RunMeta],
    run_ids: &BTreeSet<&str>,
    branch: &str,
) -> Result<StartingRefs, String> {
    let mut root_base_specs: Vec<(&str, &str, String)> = Vec::new();
    for run in ordered {
        if parent_run_id(run, run_ids).is_some() {
            continue;
        }
        let spec = run
            .base_sha
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("run {} has no base_sha in start.json", run.id))?;
        root_base_specs.push((run.id.as_str(), spec, format!("{spec}^{{commit}}")));
    }

    if root_base_specs.is_empty() {
        return Err("no root run found to determine base commit".to_string());
    }

    let mut run_branch_specs: Vec<(&str, String, String)> = Vec::with_capacity(ordered.len());
    for run in ordered {
        let run_branch = match run.branch_hint.as_deref() {
            Some(b) => b.to_string(),
            None => format!("wip/{}", run.id),
        };
        let rev = format!("refs/heads/{run_branch}^{{commit}}");
        run_branch_specs.push((run.id.as_str(), run_branch, rev));
    }
    let combined_branch_rev = format!("refs/heads/{branch}^{{commit}}");

    let mut unique_revs: Vec<&str> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (_, _, rev) in &root_base_specs {
        if seen.insert(rev.as_str()) {
            unique_revs.push(rev.as_str());
        }
    }
    for (_, _, rev) in &run_branch_specs {
        if seen.insert(rev.as_str()) {
            unique_revs.push(rev.as_str());
        }
    }
    if seen.insert(combined_branch_rev.as_str()) {
        unique_revs.push(combined_branch_rev.as_str());
    }

    let resolved = git_batch_resolve_commits(root, &unique_revs)?;

    let mut root_bases: Vec<(String, String)> = Vec::with_capacity(root_base_specs.len());
    for (run_id, spec, rev) in &root_base_specs {
        let sha = match resolved.get(rev.as_str()).and_then(|v| v.as_ref()) {
            Some(sha) => sha.clone(),
            None => {
                let detail = git_rev_parse(root, rev)
                    .err()
                    .unwrap_or_else(|| format!("Needed a single revision: {rev}"));
                return Err(format!(
                    "cannot resolve base_sha {spec} for run {run_id}: {detail}"
                ));
            }
        };
        root_bases.push(((*run_id).to_string(), sha));
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
    let group_base = first_sha.clone();

    let mut tips: HashMap<String, String> = HashMap::with_capacity(run_branch_specs.len());
    let mut combined_runs: Vec<CombinedRun> = Vec::with_capacity(run_branch_specs.len());
    for (run_id, run_branch, rev) in &run_branch_specs {
        let tip = match resolved.get(rev.as_str()).and_then(|v| v.as_ref()) {
            Some(sha) => sha.clone(),
            None => {
                let detail = git_rev_parse(root, rev)
                    .err()
                    .unwrap_or_else(|| format!("Needed a single revision: {rev}"));
                return Err(format!(
                    "cannot resolve tip commit for run {run_id} on branch {run_branch}: {detail}"
                ));
            }
        };
        tips.insert((*run_id).to_string(), tip.clone());
        combined_runs.push(CombinedRun {
            id: (*run_id).to_string(),
            tip,
        });
    }

    let existing_branch_tip = resolved
        .get(combined_branch_rev.as_str())
        .and_then(|v| v.clone());

    Ok(StartingRefs {
        group_base,
        tips,
        combined_runs,
        existing_branch_tip,
    })
}

fn git_batch_resolve_commits(
    dir: &Path,
    revs: &[&str],
) -> Result<HashMap<String, Option<String>>, String> {
    let mut resolved = HashMap::with_capacity(revs.len());
    let mut query_revs: Vec<&str> = Vec::with_capacity(revs.len());
    for &rev in revs {
        if rev.contains(['\n', '\r', ' ', '\t']) {
            resolved.insert(rev.to_string(), None);
        } else {
            query_revs.push(rev);
        }
    }
    if query_revs.is_empty() {
        return Ok(resolved);
    }

    let mut child = Command::new("git")
        .args(["cat-file", "--batch-check=%(objecttype) %(objectname)"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to execute git cat-file --batch-check: {e}"))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "failed to open stdin for git cat-file".to_string())?;
        for rev in &query_revs {
            writeln!(stdin, "{rev}")
                .map_err(|e| format!("failed to write to git cat-file stdin: {e}"))?;
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for git cat-file: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(format!("git cat-file --batch-check failed: {detail}"));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() != query_revs.len() {
        return Err(format!(
            "git cat-file --batch-check returned {} lines for {} revisions",
            lines.len(),
            query_revs.len()
        ));
    }
    for (rev, line) in query_revs.iter().zip(lines) {
        let sha = line
            .trim()
            .strip_prefix("commit ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        resolved.insert((*rev).to_string(), sha);
    }
    Ok(resolved)
}

fn load_gate_config(
    root: &Path,
    main_checkout: &Path,
) -> Result<crate::config::DispatchConfig, String> {
    if let Some(cfg) = crate::config::load(root)? {
        return Ok(cfg.dispatch);
    }
    if main_checkout != root {
        if let Some(cfg) = crate::config::load(main_checkout)? {
            return Ok(cfg.dispatch);
        }
    }
    Err("no blyte.toml found in repository".to_string())
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

fn worktree_git_dir(worktree_dir: &Path) -> Option<PathBuf> {
    let dot_git = worktree_dir.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let content = std::fs::read_to_string(&dot_git).ok()?;
    let rest = content.trim().strip_prefix("gitdir:")?.trim();
    if rest.is_empty() {
        return None;
    }
    let path = PathBuf::from(rest);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(worktree_dir.join(path))
    }
}

fn has_cherry_pick_head(worktree_dir: &Path) -> bool {
    worktree_git_dir(worktree_dir).is_some_and(|dir| dir.join("CHERRY_PICK_HEAD").exists())
}

fn prepare_worktree(
    root: &Path,
    worktree_dir: &Path,
    branch: &str,
    base_commit: &str,
    actual_existing_tip: Option<&str>,
    expected_existing_tip: Option<&str>,
) -> Result<(), String> {
    let wt_str = worktree_dir
        .to_str()
        .ok_or_else(|| format!("non-UTF-8 worktree path: {}", worktree_dir.display()))?;

    if let Some(actual) = actual_existing_tip {
        let expected = expected_existing_tip.ok_or_else(|| {
            format!(
                "branch {branch} already exists without an expected commit from a combine record; refusing to delete it"
            )
        })?;
        if actual != expected {
            return Err(format!(
                "branch {branch} has changed since blyte combined it (expected {expected}, actual {actual}); refusing to delete it"
            ));
        }
    }

    let rebuilt = worktree_dir.exists() || actual_existing_tip.is_some();
    if worktree_dir.exists() {
        if has_cherry_pick_head(worktree_dir) {
            let _ = git_cmd(worktree_dir, &["cherry-pick", "--abort"]);
        }
        let _ = git_cmd(root, &["worktree", "remove", "--force", wt_str]);
        if worktree_dir.exists() {
            std::fs::remove_dir_all(worktree_dir).map_err(|e| {
                format!(
                    "failed to clean existing worktree {}: {e}",
                    worktree_dir.display()
                )
            })?;
        }
    }
    if rebuilt {
        let _ = git_cmd(root, &["worktree", "prune"]);
    }

    if let Err(first_err) = git_cmd(
        root,
        &["worktree", "add", "-B", branch, wt_str, base_commit],
    ) {
        if !rebuilt {
            let _ = git_cmd(root, &["worktree", "prune"]);
            git_cmd(
                root,
                &["worktree", "add", "-B", branch, wt_str, base_commit],
            )
            .map_err(|e| format!("failed to create combine worktree {wt_str}: {e}"))?;
        } else {
            return Err(format!(
                "failed to create combine worktree {wt_str}: {first_err}"
            ));
        }
    }
    Ok(())
}

fn ensure_wt_excluded(common_dir: Option<&Path>) {
    let Some(common_dir) = common_dir else {
        return;
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

fn git_log_commits_and_tree(
    dir: &Path,
    base: &str,
) -> Result<(Vec<String>, Option<String>), String> {
    let range = format!("{base}..HEAD");
    let out = git_cmd(dir, &["log", "--reverse", "--format=%H%x20%T", &range])?;
    let mut commits = Vec::new();
    let mut last_tree = None;
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (commit_sha, tree_sha) = trimmed
            .split_once(' ')
            .ok_or_else(|| format!("unexpected git log output line: {trimmed}"))?;
        commits.push(commit_sha.to_string());
        last_tree = Some(tree_sha.to_string());
    }
    Ok((commits, last_tree))
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

fn git_cherry_pick_x(
    worktree_dir: &Path,
    run_id: &str,
    sha: &str,
) -> Result<Result<(), Vec<String>>, String> {
    let out = Command::new("git")
        .args(["cherry-pick", "-x", sha])
        .current_dir(worktree_dir)
        .output()
        .map_err(|e| format!("failed to execute git cherry-pick -x {sha} for run {run_id}: {e}"))?;
    if out.status.success() {
        return Ok(Ok(()));
    }
    let cp_stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let cp_stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let cp_detail = if !cp_stderr.is_empty() {
        cp_stderr
    } else {
        cp_stdout
    };

    let diff_out = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .current_dir(worktree_dir)
        .output()
        .map_err(|e| {
            format!(
                "git cherry-pick -x {sha} for run {run_id} failed ({cp_detail}); failed to list unmerged files: {e}"
            )
        })?;
    if !diff_out.status.success() {
        let diff_stderr = String::from_utf8_lossy(&diff_out.stderr).trim().to_string();
        return Err(format!(
            "git cherry-pick -x {sha} for run {run_id} failed ({cp_detail}); git diff --name-only --diff-filter=U failed: {diff_stderr}"
        ));
    }
    let unmerged: Vec<String> = String::from_utf8_lossy(&diff_out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    if unmerged.is_empty() {
        return Err(format!(
            "git cherry-pick -x {sha} for run {run_id} failed without unmerged files: {cp_detail}"
        ));
    }
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
    fn a_failure_on_stdout_survives_many_stderr_lines() {
        let gate = r#"echo "test thing ... FAILED"; i=0; while [ $i -lt 120 ]; do echo "Running tests/t$i" >&2; i=$((i+1)); done; exit 1"#;
        let (dir, _base_sha) = init_repo(gate);
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-stdout",
            "main",
            100,
            &[("a.txt", "hello\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-stdout", &mut out, &mut err);
        assert_ne!(code, 0);

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-stdout.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::CheckFailed);
        let output_tail = record.output_tail.as_deref().unwrap_or("");
        assert!(
            output_tail.contains("test thing ... FAILED"),
            "output_tail lost stdout failure:\n{output_tail}"
        );
    }

    #[test]
    fn every_failing_step_is_named() {
        let step1 = "echo first-step-boom && exit 1";
        let step2 = "echo second-step-boom && exit 1";
        let (dir, _base_sha) = init_repo(&format!("{step1}\n{step2}"));
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-both",
            "main",
            100,
            &[("a.txt", "hello\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-both", &mut out, &mut err);
        assert_ne!(code, 0);

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-both.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::CheckFailed);
        let failing_step = record.failing_step.as_deref().unwrap_or("");
        assert!(
            failing_step.contains(step1) && failing_step.contains(step2),
            "expected both failing steps in: {failing_step:?}"
        );
        assert_eq!(failing_step, format!("{step1}; {step2}"));
        let output_tail = record.output_tail.as_deref().unwrap_or("");
        assert!(
            output_tail.contains("first-step-boom") && output_tail.contains("second-step-boom"),
            "expected both outputs in:\n{output_tail}"
        );
    }

    #[test]
    fn unfinished_and_unverified_runs_are_refused_while_redone_is_left_out() {
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
        assert_eq!(code, 2);

        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T1") && err_str.contains("unfinished"),
            "{err_str}"
        );
        assert!(
            err_str.contains("T2") && err_str.contains("gate_failed"),
            "{err_str}"
        );
        assert!(!err_str.contains("T3"), "{err_str}");
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

    #[test]
    fn unparseable_start_file_errors_with_file_name() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-bad-start",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        std::fs::write(root.join(".wt/broken.start.json"), "{not valid json").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-bad-start", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("broken.start.json"), "{err_str}");
    }

    #[test]
    fn start_file_with_mismatched_id_errors_naming_both() {
        let (dir, base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-id-mismatch",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        let bad_start = serde_json::json!({
            "id": "T99",
            "request": "req-id-mismatch",
            "base": "main",
            "base_sha": base_sha,
            "started_at": 100,
            "research": false,
        });
        std::fs::write(
            root.join(".wt/T1.start.json"),
            serde_json::to_string_pretty(&bad_start).unwrap(),
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-id-mismatch", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T1") && err_str.contains("T99") && err_str.contains("T1.start.json"),
            "{err_str}"
        );
    }

    #[test]
    fn start_file_without_started_at_errors_with_file_name() {
        let (dir, base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-no-ts",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        let no_ts_start = serde_json::json!({
            "id": "T1",
            "request": "req-no-ts",
            "base": "main",
            "base_sha": base_sha,
            "research": false,
        });
        std::fs::write(
            root.join(".wt/T1.start.json"),
            serde_json::to_string_pretty(&no_ts_start).unwrap(),
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-no-ts", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("T1.start.json"), "{err_str}");
    }

    #[test]
    fn unparseable_summary_file_errors_with_file_name() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-bad-summary",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        std::fs::write(root.join(".wt/T1.json"), "{broken summary").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-bad-summary", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = std::str::from_utf8(&err).unwrap();
        assert!(err_str.contains("T1.json"), "{err_str}");
        assert!(!err_str.contains("unfinished"), "{err_str}");

        std::fs::write(root.join(".wt/T1.json"), r#"{"id":"T1","branch":"wip/T1"}"#).unwrap();
        out.clear();
        err.clear();
        let code = combine_in(root, "req-bad-summary", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = std::str::from_utf8(&err).unwrap();
        assert!(err_str.contains("T1.json"), "{err_str}");
    }

    #[test]
    fn unparseable_verdict_file_errors_with_file_name() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-bad-verdict",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        std::fs::write(root.join(".wt/T1.verdict.json"), "not json").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-bad-verdict", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = std::str::from_utf8(&err).unwrap();
        assert!(err_str.contains("T1.verdict.json"), "{err_str}");

        std::fs::write(root.join(".wt/T1.verdict.json"), r#"{"notes":"missing"}"#).unwrap();
        out.clear();
        err.clear();
        let code = combine_in(root, "req-bad-verdict", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = std::str::from_utf8(&err).unwrap();
        assert!(err_str.contains("T1.verdict.json"), "{err_str}");
    }

    #[test]
    fn root_run_with_base_but_no_base_sha_errors_naming_run() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-no-sha",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        let start_without_sha = serde_json::json!({
            "id": "T1",
            "request": "req-no-sha",
            "base": "main",
            "started_at": 100,
            "research": false,
        });
        std::fs::write(
            root.join(".wt/T1.start.json"),
            serde_json::to_string_pretty(&start_without_sha).unwrap(),
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-no-sha", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("T1"), "{err_str}");
    }

    #[test]
    fn missing_run_branch_errors_even_when_worktree_head_exists() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-no-branch",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        let t1_wt = root.join(".wt/T1");
        git(&t1_wt, &["checkout", "--detach", "HEAD"]);
        git(root, &["branch", "-D", "wip/T1"]);
        assert!(t1_wt.is_dir());
        assert!(!git(&t1_wt, &["rev-parse", "HEAD"]).is_empty());

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-no-branch", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T1") && err_str.contains("wip/T1"),
            "{err_str}"
        );
    }

    #[test]
    fn empty_cherry_pick_fails_with_exit_2_not_conflict() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-empty-pick",
            "main",
            100,
            &[("shared.txt", "identical change\n", "T1 identical")],
            Some("verified"),
        );
        let tip_t2 = create_run_branch(
            root,
            "T2",
            "req-empty-pick",
            "main",
            200,
            &[("shared.txt", "identical change\n", "T2 identical")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-empty-pick", &mut out, &mut err);
        assert_eq!(code, 2, "stderr: {}", String::from_utf8_lossy(&err));
        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T2") && err_str.contains(&tip_t2),
            "{err_str}"
        );
        assert!(!root.join(".wt/combined/req-empty-pick.json").exists());

        let combine_wt = root.join(".wt/combine-req-empty-pick");
        let status = git(&combine_wt, &["status", "--porcelain"]);
        assert!(status.is_empty(), "worktree should be clean: {status}");
    }

    #[test]
    fn manually_modified_combined_branch_refuses_rebuild_and_preserves_commit() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-moved",
            "main",
            100,
            &[("a.txt", "v1\n", "v1")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(combine_in(root, "req-moved", &mut out, &mut err), 0);
        let combined_sha = git(root, &["rev-parse", "blyte/req-moved"]);

        let combine_wt = root.join(".wt/combine-req-moved");
        std::fs::write(combine_wt.join("manual.txt"), "hand-made\n").unwrap();
        git(&combine_wt, &["add", "manual.txt"]);
        git(
            &combine_wt,
            &["commit", "--no-gpg-sign", "-m", "hand-made commit"],
        );
        let hand_sha = git(root, &["rev-parse", "blyte/req-moved"]);
        assert_ne!(hand_sha, combined_sha);

        let t1_wt = root.join(".wt/T1");
        std::fs::write(t1_wt.join("a.txt"), "v2\n").unwrap();
        git(&t1_wt, &["add", "a.txt"]);
        git(&t1_wt, &["commit", "--no-gpg-sign", "-m", "v2"]);

        out.clear();
        err.clear();
        let code = combine_in(root, "req-moved", &mut out, &mut err);
        assert_eq!(code, 2, "stderr: {}", String::from_utf8_lossy(&err));
        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("blyte/req-moved")
                && err_str.contains(&combined_sha)
                && err_str.contains(&hand_sha),
            "{err_str}"
        );
        assert_eq!(git(root, &["rev-parse", "blyte/req-moved"]), hand_sha);
        assert!(combine_wt.join("manual.txt").exists());
    }

    #[test]
    fn redone_runs_are_left_out_regardless_of_outcome() {
        let (dir, _base_sha) =
            init_repo("test -f a.txt && test ! -f b.txt && test ! -f c.txt && test ! -f d.txt");
        let root = dir.path();
        let tip_t1 = create_run_branch(
            root,
            "T1",
            "req-redone-mix",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        create_run_branch(
            root,
            "T2",
            "req-redone-mix",
            "main",
            200,
            &[("b.txt", "b\n", "add b")],
            Some("verified"),
        );
        create_run_branch(
            root,
            "T3",
            "req-redone-mix",
            "main",
            300,
            &[("c.txt", "c\n", "add c")],
            Some("gate_failed"),
        );
        create_run_branch(
            root,
            "T4",
            "req-redone-mix",
            "main",
            400,
            &[("d.txt", "d\n", "add d")],
            None,
        );
        for id in ["T2", "T3", "T4"] {
            std::fs::write(
                root.join(format!(".wt/{id}.verdict.json")),
                r#"{"verdict":"redone","notes":"replaced","ts":10}"#,
            )
            .unwrap();
        }

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-redone-mix", &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));

        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-redone-mix.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record.state, CombineState::Combined);
        assert_eq!(
            record.runs,
            vec![CombinedRun {
                id: "T1".to_string(),
                tip: tip_t1,
            }]
        );
        let combine_wt = root.join(".wt/combine-req-redone-mix");
        assert!(combine_wt.join("a.txt").exists());
        assert!(!combine_wt.join("b.txt").exists());
        assert!(!combine_wt.join("c.txt").exists());
        assert!(!combine_wt.join("d.txt").exists());
    }

    #[test]
    fn run_based_on_left_out_run_errors_naming_both() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-dep-redone",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        create_run_branch(
            root,
            "T2",
            "req-dep-redone",
            "wip/T1",
            200,
            &[("b.txt", "b\n", "add b")],
            Some("verified"),
        );
        std::fs::write(
            root.join(".wt/T1.verdict.json"),
            r#"{"verdict":"redone","notes":"replaced","ts":10}"#,
        )
        .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-dep-redone", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(
            err_str.contains("T1") && err_str.contains("T2"),
            "{err_str}"
        );
        assert!(!root.join(".wt/combined/req-dep-redone.json").exists());
    }

    #[test]
    fn every_run_redone_exits_2_saying_so() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        create_run_branch(
            root,
            "T1",
            "req-all-redone",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        create_run_branch(
            root,
            "T2",
            "req-all-redone",
            "main",
            200,
            &[("b.txt", "b\n", "add b")],
            Some("gate_failed"),
        );
        for id in ["T1", "T2"] {
            std::fs::write(
                root.join(format!(".wt/{id}.verdict.json")),
                r#"{"verdict":"redone","notes":"replaced","ts":10}"#,
            )
            .unwrap();
        }

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = combine_in(root, "req-all-redone", &mut out, &mut err);
        assert_eq!(code, 2);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("every run was redone"), "{err_str}");
        assert!(!root.join(".wt/combined/req-all-redone.json").exists());
    }

    #[test]
    fn recording_redone_on_combined_run_rebuilds_without_it() {
        let (dir, _base_sha) = init_repo("true");
        let root = dir.path();
        let tip_t1 = create_run_branch(
            root,
            "T1",
            "req-recombine",
            "main",
            100,
            &[("a.txt", "a\n", "add a")],
            Some("verified"),
        );
        create_run_branch(
            root,
            "T2",
            "req-recombine",
            "main",
            200,
            &[("b.txt", "b\n", "add b")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(combine_in(root, "req-recombine", &mut out, &mut err), 0);
        let first_head = git(root, &["rev-parse", "blyte/req-recombine"]);
        let combine_wt = root.join(".wt/combine-req-recombine");
        assert!(combine_wt.join("a.txt").exists());
        assert!(combine_wt.join("b.txt").exists());

        std::fs::write(
            root.join(".wt/T2.verdict.json"),
            r#"{"verdict":"redone","notes":"replaced","ts":10}"#,
        )
        .unwrap();

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-recombine", &mut out, &mut err), 0);
        let second_head = git(root, &["rev-parse", "blyte/req-recombine"]);
        assert_ne!(second_head, first_head);
        let record: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-recombine.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            record.runs,
            vec![CombinedRun {
                id: "T1".to_string(),
                tip: tip_t1,
            }]
        );
        assert!(combine_wt.join("a.txt").exists());
        assert!(!combine_wt.join("b.txt").exists());
    }

    #[test]
    fn followup_request_based_on_combined_branch_combines_onto_it() {
        let marker = tempfile::NamedTempFile::new().unwrap();
        let marker_path = marker.path().to_path_buf();
        let gate = format!(
            "test -f a.txt && (if test -f b.txt; then echo ab; else echo a; fi) >> '{}'",
            marker_path.display()
        );
        let (dir, base_sha) = init_repo(&gate);
        let root = dir.path();

        create_run_branch(
            root,
            "A1",
            "req-a",
            "main",
            100,
            &[("a.txt", "from a\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code_a = combine_in(root, "req-a", &mut out, &mut err);
        assert_eq!(code_a, 0, "stderr: {}", String::from_utf8_lossy(&err));
        let a_tip = git(root, &["rev-parse", "blyte/req-a"]);
        let a_record_before =
            std::fs::read_to_string(root.join(".wt/combined/req-a.json")).unwrap();

        let tip_b1 = create_run_branch(
            root,
            "B1",
            "req-b",
            "blyte/req-a",
            200,
            &[("b.txt", "from b\n", "add b")],
            Some("verified"),
        );

        out.clear();
        err.clear();
        let code_b = combine_in(root, "req-b", &mut out, &mut err);
        assert_eq!(code_b, 0, "stderr: {}", String::from_utf8_lossy(&err));

        let record_b: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-b.json")).unwrap(),
        )
        .unwrap();
        let b_tip = git(root, &["rev-parse", "blyte/req-b"]);
        assert_eq!(record_b.state, CombineState::Combined);
        assert_eq!(record_b.branch, "blyte/req-b");
        assert_eq!(record_b.base, a_tip);
        assert_eq!(
            record_b.runs,
            vec![CombinedRun {
                id: "B1".to_string(),
                tip: tip_b1.clone(),
            }]
        );
        assert_eq!(record_b.commits, vec![b_tip.clone()]);

        let history: Vec<String> = git(root, &["rev-list", "--reverse", "blyte/req-b"])
            .lines()
            .map(ToString::to_string)
            .collect();
        assert_eq!(history, vec![base_sha, a_tip.clone(), b_tip.clone()]);

        let subjects = git(root, &["log", "--reverse", "--format=%s", "blyte/req-b"]);
        assert_eq!(subjects, "initial\nadd a\nadd b");

        let b_commit_body = git(root, &["log", "-n", "1", "--format=%B", "blyte/req-b"]);
        assert!(b_commit_body.contains(&format!("(cherry picked from commit {tip_b1})")));

        assert_eq!(git(root, &["rev-parse", "blyte/req-a"]), a_tip);
        assert_eq!(
            std::fs::read_to_string(root.join(".wt/combined/req-a.json")).unwrap(),
            a_record_before
        );

        let combine_b_wt = root.join(".wt/combine-req-b");
        assert!(combine_b_wt.join("a.txt").exists());
        assert!(combine_b_wt.join("b.txt").exists());

        let gate_runs: Vec<String> = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .map(ToString::to_string)
            .collect();
        assert_eq!(gate_runs, vec!["a".to_string(), "ab".to_string()]);
    }

    #[test]
    fn followup_request_combining_second_time_changes_nothing() {
        let marker = tempfile::NamedTempFile::new().unwrap();
        let marker_path = marker.path().to_path_buf();
        let gate = format!("echo run >> '{}'", marker_path.display());
        let (dir, _base_sha) = init_repo(&gate);
        let root = dir.path();

        create_run_branch(
            root,
            "A1",
            "req-a",
            "main",
            100,
            &[("a.txt", "from a\n", "add a")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(combine_in(root, "req-a", &mut out, &mut err), 0);

        create_run_branch(
            root,
            "B1",
            "req-b",
            "blyte/req-a",
            200,
            &[("b.txt", "from b\n", "add b")],
            Some("verified"),
        );

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-b", &mut out, &mut err), 0);
        let first_tip = git(root, &["rev-parse", "blyte/req-b"]);
        let first_record_raw =
            std::fs::read_to_string(root.join(".wt/combined/req-b.json")).unwrap();
        let first_record: CombineRecord = serde_json::from_str(&first_record_raw).unwrap();
        let gate_runs_first = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();
        assert_eq!(gate_runs_first, 2);

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-b", &mut out, &mut err), 0);
        let second_tip = git(root, &["rev-parse", "blyte/req-b"]);
        let second_record_raw =
            std::fs::read_to_string(root.join(".wt/combined/req-b.json")).unwrap();
        let second_record: CombineRecord = serde_json::from_str(&second_record_raw).unwrap();
        let stdout_record: CombineRecord = serde_json::from_slice(&out).unwrap();
        let gate_runs_second = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();

        assert_eq!(second_tip, first_tip);
        assert_eq!(second_record, first_record);
        assert_eq!(second_record_raw, first_record_raw);
        assert_eq!(stdout_record, first_record);
        assert_eq!(gate_runs_second, 2);
    }

    #[test]
    fn followup_request_stays_pinned_after_base_request_gains_new_commit() {
        let marker = tempfile::NamedTempFile::new().unwrap();
        let marker_path = marker.path().to_path_buf();
        let gate = format!("test -f a1.txt && echo run >> '{}'", marker_path.display());
        let (dir, base_sha) = init_repo(&gate);
        let root = dir.path();

        create_run_branch(
            root,
            "A1",
            "req-a",
            "main",
            100,
            &[("a1.txt", "a1\n", "add a1")],
            Some("verified"),
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(combine_in(root, "req-a", &mut out, &mut err), 0);
        let a_tip_v1 = git(root, &["rev-parse", "blyte/req-a"]);

        create_run_branch(
            root,
            "B1",
            "req-b",
            "blyte/req-a",
            200,
            &[("b1.txt", "b1\n", "add b1")],
            Some("verified"),
        );

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-b", &mut out, &mut err), 0);
        let b_tip_before = git(root, &["rev-parse", "blyte/req-b"]);
        let b_record_before: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-b.json")).unwrap(),
        )
        .unwrap();

        create_run_branch(
            root,
            "A2",
            "req-a",
            "main",
            300,
            &[("a2.txt", "a2\n", "add a2")],
            Some("verified"),
        );

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-a", &mut out, &mut err), 0);
        let a_tip_v2 = git(root, &["rev-parse", "blyte/req-a"]);
        assert_ne!(a_tip_v2, a_tip_v1);
        let a_subjects = git(root, &["log", "--reverse", "--format=%s", "blyte/req-a"]);
        assert_eq!(a_subjects, "initial\nadd a1\nadd a2");

        assert_eq!(git(root, &["rev-parse", "blyte/req-b"]), b_tip_before);
        let b_history_before: Vec<String> = git(root, &["rev-list", "--reverse", "blyte/req-b"])
            .lines()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            b_history_before,
            vec![base_sha.clone(), a_tip_v1.clone(), b_tip_before.clone()]
        );
        assert!(!root.join(".wt/combine-req-b/a2.txt").exists());

        let gate_runs_before_recombine = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();
        assert_eq!(gate_runs_before_recombine, 3);

        out.clear();
        err.clear();
        assert_eq!(combine_in(root, "req-b", &mut out, &mut err), 0);
        let b_tip_after = git(root, &["rev-parse", "blyte/req-b"]);
        let b_record_after: CombineRecord = serde_json::from_str(
            &std::fs::read_to_string(root.join(".wt/combined/req-b.json")).unwrap(),
        )
        .unwrap();
        let b_history_after: Vec<String> = git(root, &["rev-list", "--reverse", "blyte/req-b"])
            .lines()
            .map(ToString::to_string)
            .collect();
        let gate_runs_after_recombine = std::fs::read_to_string(&marker_path)
            .unwrap()
            .lines()
            .count();

        assert_eq!(b_tip_after, b_tip_before);
        assert_eq!(b_record_after, b_record_before);
        assert_eq!(b_record_after.base, a_tip_v1);
        assert_eq!(b_history_after, vec![base_sha, a_tip_v1, b_tip_before]);
        assert_eq!(git(root, &["rev-parse", "blyte/req-a"]), a_tip_v2);
        assert_eq!(gate_runs_after_recombine, 3);
    }
}
