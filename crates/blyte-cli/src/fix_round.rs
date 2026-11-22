use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model_review::ModelReviewResult;

/// Exact commit message instructed for a fix round.
pub const COMMIT_MESSAGE: &str = "fix: fail loudly where review found a fallback";

/// A confirmed fallback sent to the implementer during a fix round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedFallback {
    pub file: String,
    pub line: usize,
    pub text: String,
    pub reason: String,
}

/// Summary fields recorded for a completed fix round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixRoundSummary {
    pub outcome: String,
    pub turns: u32,
    pub cost_usd: Option<f64>,
    pub seconds: u64,
    pub commits: Vec<String>,
    /// Confirmed fallbacks in the review that stands; `None` (null) when that review did not
    /// run or failed, so the count is not known.
    pub fallbacks_left: Option<usize>,
    pub kept: bool,
}

/// Persistent state file `.wt/<id>.fixround.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FixRoundRecord {
    Started {
        pre_round_sha: String,
        fallbacks_sent: Vec<ConfirmedFallback>,
        started_at: u64,
    },
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_round_sha: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fallbacks_sent: Vec<ConfirmedFallback>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<u64>,
        #[serde(flatten)]
        summary: FixRoundSummary,
    },
}

/// Path to `.wt/<id>.fixround.json`.
pub fn record_path(wt_parent: &Path, id: &str) -> PathBuf {
    wt_parent.join(format!("{id}.fixround.json"))
}

/// Read and parse `.wt/<id>.fixround.json`. Returns `Ok(None)` only when the file does not exist.
pub fn load_record(path: &Path) -> Result<Option<FixRoundRecord>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("cannot parse {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Write `.wt/<id>.fixround.json`.
pub fn write_record(path: &Path, record: &FixRoundRecord) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("cannot serialize {}: {e}", path.display()))?;
    std::fs::write(path, format!("{json}\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Count verdicts equal to `"fallback"` in a model review result.
pub fn confirmed_count(review: &ModelReviewResult) -> usize {
    review
        .fallbacks
        .iter()
        .filter(|f| f.verdict == "fallback")
        .count()
}

/// Count confirmed fallbacks in the standing review; `None` when no review ran or it failed.
pub fn fallbacks_left(review: Option<&ModelReviewResult>) -> Option<usize> {
    match review {
        Some(mr) if !mr.is_failed() => Some(confirmed_count(mr)),
        _ => None,
    }
}

/// Decide whether a fix round should run after the first pass and its review.
pub fn should_run(
    is_research: bool,
    outcome: &str,
    fix_round_enabled: bool,
    review: Option<&ModelReviewResult>,
) -> bool {
    if is_research || outcome != "verified" || !fix_round_enabled {
        return false;
    }
    let Some(mr) = review else {
        return false;
    };
    if mr.is_failed() {
        return false;
    }
    confirmed_count(mr) > 0
}

/// Build the task text passed to the implementer for a fix round.
pub fn task_text(original_task: &str, fallbacks: &[ConfirmedFallback]) -> String {
    let mut out = String::new();
    out.push_str(
        "This work passed its check, and a review found the fallbacks listed below.\n\
         For each one, change the code so that the missing, unknown or unreadable case fails with an error that names what is missing, and update or add a test for that case.\n\
         If a listed line is not a fallback, leave it and say why in the final message.\n\
         Change nothing else.\n\
         Commit with the message `fix: fail loudly where review found a fallback`.\n\n",
    );
    for fb in fallbacks {
        out.push_str(&format!("{}:{} {}\n", fb.file, fb.line, fb.text));
        out.push_str(&format!("  {}\n", fb.reason));
    }
    out.push_str("\n## Original task (for context; already done)\n\n");
    out.push_str(original_task);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Extract confirmed fallbacks from a succeeded model review and its checks file or worktree.
pub fn collect_confirmed_fallbacks(
    root: &Path,
    id: &str,
    worktree_dir: &Path,
    review: &ModelReviewResult,
) -> Result<Vec<ConfirmedFallback>, String> {
    let checks_path = root.join(".wt").join(format!("{id}.checks.json"));
    let checks_raw = std::fs::read_to_string(&checks_path)
        .map_err(|e| format!("cannot read {}: {e}", checks_path.display()))?;
    let checks_val: serde_json::Value = serde_json::from_str(&checks_raw)
        .map_err(|e| format!("cannot parse {}: {e}", checks_path.display()))?;
    let checks_fallbacks = checks_val
        .get("fallbacks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("missing \"fallbacks\" array in {}", checks_path.display()))?;

    let mut out = Vec::new();
    for v in review.fallbacks.iter().filter(|f| f.verdict == "fallback") {
        let matched_text = checks_fallbacks.iter().find_map(|entry| {
            let file = entry.get("file")?.as_str()?;
            let line = usize::try_from(entry.get("line")?.as_u64()?).ok()?;
            if file == v.file && line == v.line {
                entry.get("text")?.as_str().map(str::to_string)
            } else {
                None
            }
        });
        let text = match matched_text {
            Some(t) => t,
            None => read_worktree_line(worktree_dir, &v.file, v.line)?,
        };
        out.push(ConfirmedFallback {
            file: v.file.clone(),
            line: v.line,
            text,
            reason: v.reason.clone(),
        });
    }
    Ok(out)
}

fn read_worktree_line(worktree_dir: &Path, file: &str, line: usize) -> Result<String, String> {
    let path = worktree_dir.join(file);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if line == 0 {
        return Err(format!("invalid line 0 in {}", path.display()));
    }
    content
        .lines()
        .nth(line - 1)
        .map(|l| l.trim().to_string())
        .ok_or_else(|| format!("line {line} not found in {}", path.display()))
}

/// Append a flag to `.wt/<id>.checks.json`.
pub fn append_check_flag(root: &Path, id: &str, flag: &str) -> Result<(), String> {
    let checks_path = root.join(".wt").join(format!("{id}.checks.json"));
    let raw = std::fs::read_to_string(&checks_path)
        .map_err(|e| format!("cannot read {}: {e}", checks_path.display()))?;
    let mut val: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse {}: {e}", checks_path.display()))?;
    let flags = val
        .get_mut("flags")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| format!("missing \"flags\" array in {}", checks_path.display()))?;
    flags.push(serde_json::Value::String(flag.to_string()));
    let updated = serde_json::to_string_pretty(&val)
        .map_err(|e| format!("cannot serialize {}: {e}", checks_path.display()))?;
    std::fs::write(&checks_path, format!("{updated}\n"))
        .map_err(|e| format!("cannot write {}: {e}", checks_path.display()))
}

/// Flag appended to the checks file when a fix round did not produce a kept change.
pub fn failure_flag(round_outcome: &str) -> String {
    if round_outcome == "no_change" {
        "fix round made no change".to_string()
    } else {
        format!("fix round did not pass: {round_outcome}")
    }
}

/// Sum optional dollar costs across two passes.
pub fn sum_costs(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Combine the first pass's final message with the fix round's when the round was kept.
pub fn combined_final_message(
    first: Option<String>,
    round: Option<String>,
    kept: bool,
) -> Option<String> {
    if !kept {
        return first;
    }
    match (first, round) {
        (Some(f), Some(r)) if !r.is_empty() => Some(format!("{f}\n\nFix round:\n{r}")),
        (None, Some(r)) if !r.is_empty() => Some(format!("Fix round:\n{r}")),
        (first, _) => first,
    }
}
