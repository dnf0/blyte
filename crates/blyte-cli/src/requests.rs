use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::cli::ReviewVerdict;
use crate::combine::{CombineRecord, CombineState, CombinedRun};
use crate::serve::Surface;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Waiting { reasons: Vec<String> },
}

pub trait Publisher: Send + Sync {
    fn push_branch(&self, root: &Path, branch: &str) -> Result<(), String>;
    fn open_pull_request(
        &self,
        root: &Path,
        branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String, String>;
}

pub struct RealPublisher;

impl Publisher for RealPublisher {
    fn push_branch(&self, root: &Path, branch: &str) -> Result<(), String> {
        crate::publish::push_branch(root, branch)
    }

    fn open_pull_request(
        &self,
        root: &Path,
        branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String, String> {
        crate::publish::open_pull_request(root, branch, title, body)
    }
}

#[derive(Default)]
struct RequestsInner {
    running: HashSet<String>,
}

pub struct RequestsState {
    inner: Mutex<RequestsInner>,
    cv: Condvar,
}

impl Default for RequestsState {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestsState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RequestsInner::default()),
            cv: Condvar::new(),
        }
    }

    fn lock_inner(&self) -> MutexGuard<'_, RequestsInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_combining(&self, request: &str) -> bool {
        self.lock_inner().running.contains(request)
    }

    pub fn wait_for_idle(&self, request: &str) {
        let mut inner = self.lock_inner();
        while inner.running.contains(request) {
            inner = self.cv.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn wait_all_idle(&self) {
        let mut inner = self.lock_inner();
        while !inner.running.is_empty() {
            inner = self.cv.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitSummary {
    pub sha: String,
    pub subject: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptResult {
    Combined,
    Conflict,
    CheckFailed,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub runs: Vec<CombinedRun>,
    pub result: AttemptResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub reported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RequestStatus {
    pub request: String,
    pub runs: Vec<String>,
    pub left_out: Vec<String>,
    pub state: String,
    pub waiting_reasons: Vec<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub commits: Vec<CommitSummary>,
    pub conflicting_run: Option<String>,
    pub conflicting_files: Vec<String>,
    pub failing_step: Option<String>,
    pub output_tail: Option<String>,
    pub error: Option<String>,
    pub suggested_title: Option<String>,
    pub suggested_body: Option<String>,
    pub pushed: bool,
    pub pull_request_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestActionError {
    BadRequest(String),
    Conflict(String),
    Internal(String),
}

struct StartRunInfo {
    id: String,
    request: Option<String>,
    base: Option<String>,
    started_at: u64,
    is_research: bool,
}

fn is_research_start(wt_dir: &Path, id: &str, val: &serde_json::Value) -> bool {
    val.get("research")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || val
            .get("tags")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| arr.iter().any(|t| t.as_str() == Some("research")))
        || wt_dir.join(format!("{id}.report.md")).exists()
}

fn parse_start_file(wt_dir: &Path, path: &Path) -> Result<StartRunInfo, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let val: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
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
    let request = match val.get("request") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(format!(
                "start file {} has non-string request field",
                path.display()
            ));
        }
    };
    let base = match val.get("base") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(format!(
                "start file {} has non-string base field",
                path.display()
            ));
        }
    };
    let is_research = is_research_start(wt_dir, file_id, &val);
    let started_at = match val.get("started_at") {
        Some(v) => v
            .as_u64()
            .ok_or_else(|| format!("start file {} has non-numeric started_at", path.display()))?,
        None => 0,
    };
    Ok(StartRunInfo {
        id: file_id.to_string(),
        request,
        base,
        started_at,
        is_research,
    })
}

fn list_start_files(wt_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let read_dir = match std::fs::read_dir(wt_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("failed to read {}: {e}", wt_dir.display())),
    };
    let mut entries = Vec::new();
    for entry in read_dir {
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
    Ok(entries)
}

pub fn run_request_id(root: &Path, run_id: &str) -> Result<Option<String>, String> {
    let wt_dir = root.join(".wt");
    let path = wt_dir.join(format!("{run_id}.start.json"));
    if !path.exists() {
        return Ok(None);
    }
    let info = parse_start_file(&wt_dir, &path)?;
    if info.is_research {
        return Ok(None);
    }
    Ok(info.request)
}

pub fn request_readiness(root: &Path, request: &str) -> Result<Readiness, String> {
    crate::dispatch::validate_request_id(request)?;
    let wt_dir = root.join(".wt");
    let entries = list_start_files(&wt_dir)?;
    let mut matched_runs = Vec::new();
    for path in entries {
        let info = parse_start_file(&wt_dir, &path)?;
        if info.is_research {
            continue;
        }
        if info.request.as_deref() == Some(request) {
            matched_runs.push(info);
        }
    }
    if matched_runs.is_empty() {
        return Ok(Readiness::Waiting {
            reasons: vec!["no runs for request".to_string()],
        });
    }
    matched_runs.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut active_runs = Vec::new();
    let mut left_out_ids: BTreeSet<String> = BTreeSet::new();
    for run in matched_runs {
        let verdict = crate::queue_actions::read_verdict(root, &run.id)?;
        if crate::combine::is_left_out_verdict(verdict) {
            left_out_ids.insert(run.id);
        } else {
            active_runs.push((run, verdict));
        }
    }

    if active_runs.is_empty() {
        return Ok(Readiness::Waiting {
            reasons: vec!["every run was redone".to_string()],
        });
    }

    let left_out_refs: BTreeSet<&str> = left_out_ids.iter().map(String::as_str).collect();
    for (run, _) in &active_runs {
        crate::combine::check_not_based_on_left_out(
            root,
            &run.id,
            run.base.as_deref(),
            &left_out_refs,
        )?;
    }

    let mut reasons = Vec::new();
    for (run, verdict) in active_runs {
        let id = &run.id;
        let summary_path = wt_dir.join(format!("{id}.json"));
        let outcome = match std::fs::read_to_string(&summary_path) {
            Ok(raw) => {
                let val: serde_json::Value = serde_json::from_str(&raw)
                    .map_err(|e| format!("failed to parse {}: {e}", summary_path.display()))?;
                let outcome = val
                    .get("outcome")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!(
                            "summary file {} is missing string outcome",
                            summary_path.display()
                        )
                    })?
                    .to_string();
                if outcome != "verified" {
                    reasons.push(format!("{id}: {outcome}"));
                }
                Some(outcome)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                reasons.push(format!("{id}: unfinished"));
                None
            }
            Err(e) => {
                return Err(format!("failed to read {}: {e}", summary_path.display()));
            }
        };

        let checks_path = wt_dir.join(format!("{id}.checks.json"));
        let (checks_recorded, flags) = crate::replay::read_checks(&checks_path)
            .map_err(|e| format!("{}: {e}", checks_path.display()))?;

        if crate::queue_actions::read_hold(root, id)?.is_some() {
            reasons.push(format!("{id}: on hold"));
        }

        if let Some(ref outcome_str) = outcome {
            if outcome_str == "verified" {
                if !checks_recorded {
                    reasons.push(format!("{id}: no checks file"));
                } else {
                    let card =
                        crate::replay::card_for_run(false, outcome_str, checks_recorded, &flags);
                    let cleared_by_verdict = matches!(
                        verdict,
                        Some(ReviewVerdict::Accepted | ReviewVerdict::Fixed)
                    );
                    if card != "clean" && !cleared_by_verdict {
                        reasons.push(format!("{id}: flagged ({})", flags.join("; ")));
                    }
                }
            }
        }
    }

    if reasons.is_empty() {
        Ok(Readiness::Ready)
    } else {
        Ok(Readiness::Waiting { reasons })
    }
}

fn combine_record_path(root: &Path, request: &str) -> PathBuf {
    root.join(".wt")
        .join("combined")
        .join(format!("{request}.json"))
}

fn attempt_record_path(root: &Path, request: &str) -> PathBuf {
    root.join(".wt")
        .join("combined")
        .join(format!("{request}.attempt.json"))
}

fn published_record_path(root: &Path, request: &str) -> PathBuf {
    root.join(".wt")
        .join("combined")
        .join(format!("{request}.published.json"))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T, label: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let mut json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("failed to serialize {label}: {e}"))?;
    json.push('\n');
    let mut tmp_name = path.as_os_str().to_os_string();
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    tmp_name.push(format!(".tmp.{}.{seq}", std::process::id()));
    let tmp_path = PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!(
            "failed to rename {} to {}: {e}",
            tmp_path.display(),
            path.display()
        )
    })
}

pub fn load_combine_record(root: &Path, request: &str) -> Result<Option<CombineRecord>, String> {
    let path = combine_record_path(root, request);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };
    let record: CombineRecord = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    Ok(Some(record))
}

pub fn load_attempt_record(root: &Path, request: &str) -> Result<Option<AttemptRecord>, String> {
    let path = attempt_record_path(root, request);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };
    let record: AttemptRecord = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    if record.result == AttemptResult::Error
        && record.error.as_deref().is_none_or(|s| s.trim().is_empty())
    {
        return Err(format!(
            "invalid attempt file {}: result error is missing error text",
            path.display()
        ));
    }
    Ok(Some(record))
}

pub fn write_attempt_record(
    root: &Path,
    request: &str,
    record: &AttemptRecord,
) -> Result<(), String> {
    let path = attempt_record_path(root, request);
    write_json_atomic(&path, record, "attempt record")
}

pub fn load_published_record(root: &Path, request: &str) -> Result<PublishedRecord, String> {
    let path = published_record_path(root, request);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(PublishedRecord::default()),
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };
    serde_json::from_str(&raw).map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

pub fn write_published_record(
    root: &Path,
    request: &str,
    record: &PublishedRecord,
) -> Result<(), String> {
    let path = published_record_path(root, request);
    write_json_atomic(&path, record, "published record")
}

fn git_stdout(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_rev_parse(root: &Path, rev: &str) -> Result<String, String> {
    git_stdout(root, &["rev-parse", "--verify", rev])
}

fn git_commit_subject(root: &Path, sha: &str) -> Result<String, String> {
    git_stdout(root, &["show", "-s", "--format=%s", sha])
        .map_err(|e| format!("failed to read subject for commit {sha}: {e}"))
}

fn resolve_current_run_tips(root: &Path, request: &str) -> Result<Vec<CombinedRun>, String> {
    let wt_dir = root.join(".wt");
    let entries = list_start_files(&wt_dir)?;
    let mut tips = BTreeMap::new();
    for path in entries {
        let info = parse_start_file(&wt_dir, &path)?;
        if info.is_research || info.request.as_deref() != Some(request) {
            continue;
        }
        if crate::combine::is_run_left_out(root, &info.id)? {
            continue;
        }
        let summary_path = wt_dir.join(format!("{}.json", info.id));
        let raw = std::fs::read_to_string(&summary_path)
            .map_err(|e| format!("failed to read {}: {e}", summary_path.display()))?;
        let val: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("failed to parse {}: {e}", summary_path.display()))?;
        let branch_hint = match val.get("branch") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(format!(
                    "summary file {} has non-string branch",
                    summary_path.display()
                ));
            }
        };
        let branch = branch_hint.unwrap_or_else(|| format!("wip/{}", info.id));
        let rev = format!("refs/heads/{branch}^{{commit}}");
        let sha = git_rev_parse(root, &rev).map_err(|e| {
            format!(
                "run {}: branch {branch} does not resolve to a commit: {e}",
                info.id
            )
        })?;
        tips.insert(info.id, sha);
    }
    Ok(tips
        .into_iter()
        .map(|(id, tip)| CombinedRun { id, tip })
        .collect())
}

fn sorted_runs(mut runs: Vec<CombinedRun>) -> Vec<CombinedRun> {
    runs.sort_by(|a, b| a.id.cmp(&b.id));
    runs
}

fn record_runs_match_tips(record: &CombineRecord, current_runs: &[CombinedRun]) -> bool {
    if record.runs.len() != current_runs.len() {
        return false;
    }
    sorted_runs(record.runs.clone()) == current_runs
}

fn combine_record_tip(record: &CombineRecord) -> &str {
    record.commits.last().unwrap_or(&record.base).as_str()
}

fn is_record_pushed(published: &PublishedRecord, record: &CombineRecord) -> bool {
    published.pushed_commit.as_deref() == Some(combine_record_tip(record))
}

fn branch_matches_record(root: &Path, record: &CombineRecord) -> bool {
    let expected = combine_record_tip(record);
    let rev = format!("refs/heads/{}^{{commit}}", record.branch);
    git_rev_parse(root, &rev).is_ok_and(|actual| actual == expected)
}

fn format_record_failure(record: &CombineRecord) -> String {
    match record.state {
        CombineState::Conflict => {
            let run = record.conflicting_run.as_deref().unwrap_or("unknown");
            let files = if record.conflicting_files.is_empty() {
                "(none listed)".to_string()
            } else {
                record.conflicting_files.join(", ")
            };
            format!(
                "Combine failed for request {} (state: conflict).\nConflicting run: {run}\nConflicting files: {files}",
                record.request
            )
        }
        CombineState::CheckFailed => {
            let step = record.failing_step.as_deref().unwrap_or("unknown");
            let tail = record.output_tail.as_deref().unwrap_or("");
            if tail.is_empty() {
                format!(
                    "Combine failed for request {} (state: check_failed).\nFailing step: {step}",
                    record.request
                )
            } else {
                format!(
                    "Combine failed for request {} (state: check_failed).\nFailing step: {step}\nOutput tail:\n{tail}",
                    record.request
                )
            }
        }
        CombineState::Combined => String::new(),
    }
}

fn format_error_failure(request: &str, err_text: &str) -> String {
    format!("Combine failed for request {request} (state: error).\nError: {err_text}")
}

struct RunningGuard {
    state: Arc<RequestsState>,
    request: String,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        let mut inner = self.state.lock_inner();
        inner.running.remove(&self.request);
        self.state.cv.notify_all();
    }
}

fn retry_failure_message(
    root: &Path,
    request: &str,
    attempt: &AttemptRecord,
) -> Result<String, String> {
    let path = attempt_record_path(root, request);
    match attempt.result {
        AttemptResult::Conflict | AttemptResult::CheckFailed => {
            let expected_state = match attempt.result {
                AttemptResult::Conflict => CombineState::Conflict,
                AttemptResult::CheckFailed => CombineState::CheckFailed,
                AttemptResult::Combined | AttemptResult::Error => unreachable!(),
            };
            match load_combine_record(root, request)? {
                Some(rec) if rec.state == expected_state => Ok(format_record_failure(&rec)),
                _ => Err(format!(
                    "attempt file {} has result {:?} without a matching combine record",
                    path.display(),
                    attempt.result
                )),
            }
        }
        AttemptResult::Error => {
            let err_text = attempt.error.as_deref().ok_or_else(|| {
                format!(
                    "attempt file {} has result error without error text",
                    path.display()
                )
            })?;
            Ok(format_error_failure(request, err_text))
        }
        AttemptResult::Combined => Err(format!(
            "cannot format failure message for combined attempt {}",
            path.display()
        )),
    }
}

fn record_and_report_failure(
    root: &Path,
    session: &crate::session::Session,
    request: &str,
    runs: Vec<CombinedRun>,
    result: AttemptResult,
    error: Option<String>,
    msg: &str,
) {
    let mut attempt = AttemptRecord {
        runs,
        result,
        error,
        reported: false,
    };
    if let Err(e) = write_attempt_record(root, request, &attempt) {
        session.record_error(&format!(
            "failed to write attempt record for {request}: {e}"
        ));
    }
    match session.queue_for_request(request, msg) {
        Ok(_) => {
            attempt.reported = true;
            if let Err(e) = write_attempt_record(root, request, &attempt) {
                session.record_error(&format!(
                    "failed to write attempt record for {request}: {e}"
                ));
            }
        }
        Err(e) => {
            session.record_error(&format!(
                "failed to queue combine failure for {request}: {e}"
            ));
        }
    }
}

fn orchestrator_busy(surface: &Surface) -> bool {
    let info = surface.session.info();
    info.busy || info.queued > 0
}

fn ready_waiting_reason(orchestrator_busy: bool) -> String {
    if orchestrator_busy {
        "the orchestrator is still working".to_string()
    } else {
        "waiting to combine".to_string()
    }
}

pub fn maybe_combine(surface: &Surface, request: &str) -> Result<bool, String> {
    crate::dispatch::validate_request_id(request)?;
    if orchestrator_busy(surface) {
        return Ok(false);
    }
    match request_readiness(&surface.root, request)? {
        Readiness::Waiting { .. } => return Ok(false),
        Readiness::Ready => {}
    }

    let current_runs = resolve_current_run_tips(&surface.root, request)?;

    {
        let mut inner = surface.requests.lock_inner();
        if inner.running.contains(request) {
            return Ok(false);
        }
        if let Some(mut attempt) = load_attempt_record(&surface.root, request)? {
            if attempt.runs == current_runs {
                if attempt.result != AttemptResult::Combined && !attempt.reported {
                    let msg = retry_failure_message(&surface.root, request, &attempt)?;
                    match surface.session.queue_for_request(request, &msg) {
                        Ok(_) => {
                            attempt.reported = true;
                            write_attempt_record(&surface.root, request, &attempt)?;
                        }
                        Err(e) => {
                            surface.session.record_error(&format!(
                                "failed to queue combine failure for {request}: {e}"
                            ));
                        }
                    }
                }
                return Ok(false);
            }
        }
        inner.running.insert(request.to_string());
    }

    let root = surface.root.clone();
    let session = surface.session.clone();
    let requests_state = Arc::clone(&surface.requests);
    let request_owned = request.to_string();

    std::thread::spawn(move || {
        let _guard = RunningGuard {
            state: Arc::clone(&requests_state),
            request: request_owned.clone(),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = crate::combine::combine_in(&root, &request_owned, &mut out, &mut err);

        match code {
            0 => {
                let runs = match load_combine_record(&root, &request_owned) {
                    Ok(Some(rec)) => sorted_runs(rec.runs),
                    Ok(None) => current_runs,
                    Err(e) => {
                        session.record_error(&format!(
                            "failed to read combine record after combine for {request_owned}: {e}"
                        ));
                        current_runs
                    }
                };
                let attempt = AttemptRecord {
                    runs,
                    result: AttemptResult::Combined,
                    error: None,
                    reported: false,
                };
                if let Err(e) = write_attempt_record(&root, &request_owned, &attempt) {
                    session.record_error(&format!(
                        "failed to write attempt record for {request_owned}: {e}"
                    ));
                }
            }
            1 => match load_combine_record(&root, &request_owned) {
                Ok(Some(rec))
                    if matches!(
                        rec.state,
                        CombineState::Conflict | CombineState::CheckFailed
                    ) =>
                {
                    let result = match rec.state {
                        CombineState::Conflict => AttemptResult::Conflict,
                        CombineState::CheckFailed => AttemptResult::CheckFailed,
                        CombineState::Combined => unreachable!(),
                    };
                    let runs = sorted_runs(rec.runs.clone());
                    let msg = format_record_failure(&rec);
                    record_and_report_failure(
                        &root,
                        &session,
                        &request_owned,
                        runs,
                        result,
                        None,
                        &msg,
                    );
                }
                Ok(_) => {
                    let err_text = String::from_utf8_lossy(&err).trim().to_string();
                    let err_text = if err_text.is_empty() {
                        format!("combine exited with code 1 without a conflict/check_failed record for {request_owned}")
                    } else {
                        err_text
                    };
                    let msg = format_error_failure(&request_owned, &err_text);
                    record_and_report_failure(
                        &root,
                        &session,
                        &request_owned,
                        current_runs,
                        AttemptResult::Error,
                        Some(err_text),
                        &msg,
                    );
                }
                Err(e) => {
                    let msg = format_error_failure(&request_owned, &e);
                    record_and_report_failure(
                        &root,
                        &session,
                        &request_owned,
                        current_runs,
                        AttemptResult::Error,
                        Some(e),
                        &msg,
                    );
                }
            },
            other => {
                let err_text = String::from_utf8_lossy(&err).trim().to_string();
                let err_text = if err_text.is_empty() {
                    format!("combine exited with status {other}")
                } else {
                    err_text
                };
                let msg = format_error_failure(&request_owned, &err_text);
                record_and_report_failure(
                    &root,
                    &session,
                    &request_owned,
                    current_runs,
                    AttemptResult::Error,
                    Some(err_text),
                    &msg,
                );
            }
        }
    });

    Ok(true)
}

fn resolve_commit_summaries(root: &Path, shas: &[String]) -> Result<Vec<CommitSummary>, String> {
    let mut commits = Vec::with_capacity(shas.len());
    for sha in shas {
        let subject = git_commit_subject(root, sha)?;
        commits.push(CommitSummary {
            sha: sha.clone(),
            subject,
        });
    }
    Ok(commits)
}

fn suggested_pr_fields(commits: &[CommitSummary]) -> (Option<String>, Option<String>) {
    if commits.is_empty() {
        return (None, None);
    }
    let title = commits[0].subject.clone();
    let body = commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (Some(title), Some(body))
}

struct RequestGroup {
    request: String,
    max_started_at: u64,
    runs: Vec<(u64, String)>,
}

pub fn list_requests(surface: &Surface) -> Result<Vec<RequestStatus>, String> {
    let wt_dir = surface.root.join(".wt");
    let entries = list_start_files(&wt_dir)?;
    let mut groups: HashMap<String, RequestGroup> = HashMap::new();
    for path in entries {
        let info = parse_start_file(&wt_dir, &path)?;
        if info.is_research {
            continue;
        }
        let Some(req_id) = info.request else {
            continue;
        };
        let entry = groups
            .entry(req_id.clone())
            .or_insert_with(|| RequestGroup {
                request: req_id,
                max_started_at: 0,
                runs: Vec::new(),
            });
        if info.started_at > entry.max_started_at {
            entry.max_started_at = info.started_at;
        }
        entry.runs.push((info.started_at, info.id));
    }

    let mut ordered: Vec<RequestGroup> = groups.into_values().collect();
    ordered.sort_by(|a, b| {
        b.max_started_at
            .cmp(&a.max_started_at)
            .then_with(|| b.request.cmp(&a.request))
    });

    let busy = orchestrator_busy(surface);
    let mut result = Vec::with_capacity(ordered.len());
    for mut group in ordered {
        group
            .runs
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let run_ids: Vec<String> = group.runs.into_iter().map(|(_, id)| id).collect();
        result.push(build_request_status(surface, group.request, run_ids, busy));
    }
    Ok(result)
}

fn build_request_status(
    surface: &Surface,
    request: String,
    runs: Vec<String>,
    orchestrator_busy: bool,
) -> RequestStatus {
    let is_combining = surface.requests.is_combining(&request);

    let record_res = load_combine_record(&surface.root, &request);
    let published_res = load_published_record(&surface.root, &request);
    let attempt_res = load_attempt_record(&surface.root, &request);
    let left_out_res: Result<Vec<String>, String> = runs
        .iter()
        .filter_map(
            |id| match crate::combine::is_run_left_out(&surface.root, id) {
                Ok(true) => Some(Ok(id.clone())),
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            },
        )
        .collect();
    let left_out = left_out_res.as_ref().ok().cloned().unwrap_or_default();

    let (pushed, pull_request_url) = match (&published_res, &record_res) {
        (Ok(p), Ok(Some(rec))) => (
            rec.state == CombineState::Combined && is_record_pushed(p, rec),
            p.pull_request_url.clone(),
        ),
        (Ok(p), _) => (false, p.pull_request_url.clone()),
        (Err(_), _) => (false, None),
    };

    let mut status = RequestStatus {
        request: request.clone(),
        runs,
        left_out,
        state: String::new(),
        waiting_reasons: Vec::new(),
        branch: None,
        base: None,
        commits: Vec::new(),
        conflicting_run: None,
        conflicting_files: Vec::new(),
        failing_step: None,
        output_tail: None,
        error: None,
        suggested_title: None,
        suggested_body: None,
        pushed,
        pull_request_url,
    };

    if is_combining {
        status.state = "combining".to_string();
        if let Ok(Some(rec)) = record_res {
            populate_record_metadata(&surface.root, &rec, &mut status);
        }
        return status;
    }

    if let Err(e) = left_out_res {
        status.state = "error".to_string();
        status.error = Some(e);
        return status;
    }

    if let Err(e) = published_res {
        status.state = "error".to_string();
        status.error = Some(e);
        return status;
    }

    let attempt = match attempt_res {
        Ok(a) => a,
        Err(e) => {
            status.state = "error".to_string();
            status.error = Some(e);
            return status;
        }
    };

    let record = match record_res {
        Ok(r) => r,
        Err(e) => {
            status.state = "error".to_string();
            status.error = Some(e);
            return status;
        }
    };

    match request_readiness(&surface.root, &request) {
        Err(e) => {
            status.state = "error".to_string();
            status.error = Some(e);
            status
        }
        Ok(Readiness::Waiting { reasons }) => {
            status.state = "waiting".to_string();
            status.waiting_reasons = reasons;
            if let Some(ref rec) = record {
                populate_record_metadata(&surface.root, rec, &mut status);
            }
            status
        }
        Ok(Readiness::Ready) => {
            let current_runs = match resolve_current_run_tips(&surface.root, &request) {
                Ok(t) => t,
                Err(e) => {
                    status.state = "error".to_string();
                    status.error = Some(e);
                    return status;
                }
            };
            if let Some(ref att) = attempt {
                if att.runs == current_runs && att.result == AttemptResult::Error {
                    status.state = "error".to_string();
                    status.error = att.error.clone();
                    return status;
                }
            }
            let Some(rec) = record else {
                status.state = "waiting".to_string();
                status.waiting_reasons = vec![ready_waiting_reason(orchestrator_busy)];
                return status;
            };
            if !record_runs_match_tips(&rec, &current_runs) {
                status.state = "waiting".to_string();
                status.waiting_reasons = vec![ready_waiting_reason(orchestrator_busy)];
                populate_record_metadata(&surface.root, &rec, &mut status);
                return status;
            }

            let commits = match resolve_commit_summaries(&surface.root, &rec.commits) {
                Ok(c) => c,
                Err(e) => {
                    status.state = "error".to_string();
                    status.error = Some(e);
                    return status;
                }
            };

            status.branch = Some(rec.branch.clone());
            status.base = Some(rec.base.clone());
            status.commits = commits;
            status.conflicting_run = rec.conflicting_run.clone();
            status.conflicting_files = rec.conflicting_files.clone();
            status.failing_step = rec.failing_step.clone();
            status.output_tail = rec.output_tail.clone();

            match rec.state {
                CombineState::Combined => {
                    if branch_matches_record(&surface.root, &rec) {
                        status.state = "combined".to_string();
                        let (title, body) = suggested_pr_fields(&status.commits);
                        status.suggested_title = title;
                        status.suggested_body = body;
                    } else {
                        status.state = "waiting".to_string();
                        status.waiting_reasons = vec![ready_waiting_reason(orchestrator_busy)];
                    }
                }
                CombineState::Conflict => {
                    status.state = "conflict".to_string();
                }
                CombineState::CheckFailed => {
                    status.state = "check_failed".to_string();
                }
            }
            status
        }
    }
}

fn populate_record_metadata(root: &Path, rec: &CombineRecord, status: &mut RequestStatus) {
    status.branch = Some(rec.branch.clone());
    status.base = Some(rec.base.clone());
    if let Ok(commits) = resolve_commit_summaries(root, &rec.commits) {
        status.commits = commits;
    }
    status.conflicting_run = rec.conflicting_run.clone();
    status.conflicting_files = rec.conflicting_files.clone();
    status.failing_step = rec.failing_step.clone();
    status.output_tail = rec.output_tail.clone();
}

pub fn push_request(surface: &Surface, request: &str) -> Result<(), RequestActionError> {
    crate::dispatch::validate_request_id(request).map_err(RequestActionError::BadRequest)?;
    if surface.requests.is_combining(request) {
        return Err(RequestActionError::Conflict(format!(
            "request {request} is currently combining"
        )));
    }
    let record = load_combine_record(&surface.root, request)
        .map_err(RequestActionError::Internal)?
        .ok_or_else(|| {
            RequestActionError::Conflict(format!("request {request} has not been combined"))
        })?;
    if record.state != CombineState::Combined {
        return Err(RequestActionError::Conflict(format!(
            "request {request} is not combined (state: {:?})",
            record.state
        )));
    }

    let mut published =
        load_published_record(&surface.root, request).map_err(RequestActionError::Internal)?;

    match surface.publisher.push_branch(&surface.root, &record.branch) {
        Ok(()) => {
            let rev = format!("refs/heads/{}^{{commit}}", record.branch);
            let branch_commit =
                git_rev_parse(&surface.root, &rev).map_err(RequestActionError::Internal)?;
            published.pushed_commit = Some(branch_commit);
            write_published_record(&surface.root, request, &published)
                .map_err(RequestActionError::Internal)?;
            if let Err(e) = surface.session.record_decision(
                request,
                "push",
                Some(&format!("pushed {}", record.branch)),
                None,
            ) {
                surface.session.record_error(&format!(
                    "failed to record push decision for {request}: {e}"
                ));
            }
            Ok(())
        }
        Err(e) => {
            surface.session.record_error(&format!(
                "push failed for {request} ({}): {e}",
                record.branch
            ));
            Err(RequestActionError::Internal(e))
        }
    }
}

pub fn open_request_pull_request(
    surface: &Surface,
    request: &str,
    title: &str,
    body: &str,
) -> Result<String, RequestActionError> {
    crate::dispatch::validate_request_id(request).map_err(RequestActionError::BadRequest)?;
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return Err(RequestActionError::BadRequest(
            "field title must not be empty".to_string(),
        ));
    }
    if surface.requests.is_combining(request) {
        return Err(RequestActionError::Conflict(format!(
            "request {request} is currently combining"
        )));
    }
    let record = load_combine_record(&surface.root, request)
        .map_err(RequestActionError::Internal)?
        .ok_or_else(|| {
            RequestActionError::Conflict(format!("request {request} has not been combined"))
        })?;
    if record.state != CombineState::Combined {
        return Err(RequestActionError::Conflict(format!(
            "request {request} is not combined (state: {:?})",
            record.state
        )));
    }

    let mut published =
        load_published_record(&surface.root, request).map_err(RequestActionError::Internal)?;
    if !is_record_pushed(&published, &record) {
        return Err(RequestActionError::Conflict(format!(
            "branch {} must be pushed before opening a pull request",
            record.branch
        )));
    }

    match surface
        .publisher
        .open_pull_request(&surface.root, &record.branch, trimmed_title, body)
    {
        Ok(url) => {
            published.pull_request_url = Some(url.clone());
            write_published_record(&surface.root, request, &published)
                .map_err(RequestActionError::Internal)?;
            if let Err(e) =
                surface
                    .session
                    .record_decision(request, "pull_request", Some(&url), None)
            {
                surface.session.record_error(&format!(
                    "failed to record pull_request decision for {request}: {e}"
                ));
            }
            Ok(url)
        }
        Err(e) => {
            surface.session.record_error(&format!(
                "pull request failed for {request} ({}): {e}",
                record.branch
            ));
            Err(RequestActionError::Internal(e))
        }
    }
}
