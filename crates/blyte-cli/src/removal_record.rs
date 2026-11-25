use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::clean::RemovalReason;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const KNOWN_REASONS: &[&str] = &[
    "merged",
    "merged_into",
    "merged_through",
    "session_merged",
    "verdict",
    "no_changes",
    "research_report",
    "research_report_in_worktree",
    "research_ended_without_report",
    "research_stopped_without_summary",
    "older_than",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalRecord {
    pub id: String,
    pub reason: String,
    pub detail: Option<String>,
    pub removed_at: u64,
}

pub fn path(root: &Path, id: &str) -> PathBuf {
    root.join(".wt").join(format!("{id}.removed.json"))
}

pub fn reason_name(reason: &RemovalReason) -> (&'static str, Option<String>) {
    match reason {
        RemovalReason::Merged => ("merged", None),
        RemovalReason::MergedInto(base) => ("merged_into", Some(base.clone())),
        RemovalReason::MergedThrough(child) => ("merged_through", Some(child.clone())),
        RemovalReason::SessionMerged(branch) => ("session_merged", Some(branch.clone())),
        RemovalReason::Verdict(verdict) => ("verdict", Some(verdict.clone())),
        RemovalReason::NoChanges => ("no_changes", None),
        RemovalReason::ResearchReport => ("research_report", None),
        RemovalReason::ResearchReportInWorktree => ("research_report_in_worktree", None),
        RemovalReason::ResearchEndedWithoutReport(outcome) => {
            ("research_ended_without_report", Some(outcome.clone()))
        }
        RemovalReason::ResearchStoppedWithoutSummary(days) => {
            ("research_stopped_without_summary", Some(days.to_string()))
        }
        RemovalReason::OlderThan(days) => ("older_than", Some(days.to_string())),
    }
}

pub fn is_merged(record: &RemovalRecord) -> bool {
    matches!(
        record.reason.as_str(),
        "merged" | "merged_into" | "merged_through" | "session_merged"
    )
}

pub fn write(root: &Path, record: &RemovalRecord) -> Result<(), String> {
    if !KNOWN_REASONS.contains(&record.reason.as_str()) {
        return Err(format!("unknown removal reason {:?}", record.reason));
    }
    let dest = path(root, &record.id);
    let parent = dest
        .parent()
        .ok_or_else(|| format!("{}: missing parent directory", dest.display()))?;
    if let Err(err) = std::fs::create_dir_all(parent) {
        return Err(format!("{}: {err}", parent.display()));
    }
    let mut json =
        serde_json::to_string(record).map_err(|err| format!("{}: {err}", dest.display()))?;
    json.push('\n');
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        "{}.removed.json.tmp.{}.{}",
        record.id,
        std::process::id(),
        seq
    ));
    if let Err(err) = std::fs::write(&tmp_path, &json) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("{}: {err}", dest.display()));
    }
    if let Err(err) = std::fs::rename(&tmp_path, &dest) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("{}: {err}", dest.display()));
    }
    Ok(())
}

pub fn record_removal(root: &Path, id: &str, reason: &RemovalReason) -> Result<(), String> {
    let removed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| format!("system clock before Unix epoch: {err}"))?
        .as_secs();
    let (reason_str, detail) = reason_name(reason);
    let record = RemovalRecord {
        id: id.to_string(),
        reason: reason_str.to_string(),
        detail,
        removed_at,
    };
    write(root, &record)
}

pub fn read(root: &Path, id: &str) -> Result<Option<RemovalRecord>, String> {
    let file_path = path(root, id);
    let content = match std::fs::read_to_string(&file_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", file_path.display())),
    };
    let record: RemovalRecord =
        serde_json::from_str(&content).map_err(|err| format!("{}: {err}", file_path.display()))?;
    if !KNOWN_REASONS.contains(&record.reason.as_str()) {
        return Err(format!(
            "{}: unknown removal reason {:?}",
            file_path.display(),
            record.reason
        ));
    }
    Ok(Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_uses_wt_removed_json() {
        let root = Path::new("/tmp/repo");
        assert_eq!(
            path(root, "T1"),
            PathBuf::from("/tmp/repo/.wt/T1.removed.json")
        );
    }

    #[test]
    fn reason_name_and_is_merged_cover_all_variants() {
        let cases = [
            (RemovalReason::Merged, "merged", None, true),
            (
                RemovalReason::MergedInto("feat/x".to_string()),
                "merged_into",
                Some("feat/x".to_string()),
                true,
            ),
            (
                RemovalReason::MergedThrough("T2".to_string()),
                "merged_through",
                Some("T2".to_string()),
                true,
            ),
            (
                RemovalReason::SessionMerged("feat/s".to_string()),
                "session_merged",
                Some("feat/s".to_string()),
                true,
            ),
            (
                RemovalReason::Verdict("redone".to_string()),
                "verdict",
                Some("redone".to_string()),
                false,
            ),
            (RemovalReason::NoChanges, "no_changes", None, false),
            (
                RemovalReason::ResearchReport,
                "research_report",
                None,
                false,
            ),
            (
                RemovalReason::ResearchReportInWorktree,
                "research_report_in_worktree",
                None,
                false,
            ),
            (
                RemovalReason::ResearchEndedWithoutReport("failed".to_string()),
                "research_ended_without_report",
                Some("failed".to_string()),
                false,
            ),
            (
                RemovalReason::ResearchStoppedWithoutSummary(3),
                "research_stopped_without_summary",
                Some("3".to_string()),
                false,
            ),
            (
                RemovalReason::OlderThan(14),
                "older_than",
                Some("14".to_string()),
                false,
            ),
        ];
        assert_eq!(cases.len(), KNOWN_REASONS.len());
        for (variant, expected_name, expected_detail, expected_merged) in cases {
            let (name, detail) = reason_name(&variant);
            assert_eq!(name, expected_name);
            assert_eq!(detail, expected_detail);
            assert!(KNOWN_REASONS.contains(&name));
            let rec = RemovalRecord {
                id: "T1".to_string(),
                reason: name.to_string(),
                detail,
                removed_at: 1_795_000_000,
            };
            assert_eq!(is_merged(&rec), expected_merged);
        }
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path(), "MISSING").unwrap(), None);
    }

    #[test]
    fn write_and_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let rec = RemovalRecord {
            id: "T1".to_string(),
            reason: "merged".to_string(),
            detail: None,
            removed_at: 1_795_000_000,
        };
        write(dir.path(), &rec).unwrap();
        assert_eq!(read(dir.path(), "T1").unwrap(), Some(rec));
    }

    #[test]
    fn read_invalid_json_errors_naming_path() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join(".wt");
        std::fs::create_dir_all(&wt).unwrap();
        let file = wt.join("BAD.removed.json");
        std::fs::write(&file, "{").unwrap();
        let err = read(dir.path(), "BAD").unwrap_err();
        assert!(
            err.contains(&file.display().to_string()),
            "expected error to contain path {}, got: {err}",
            file.display()
        );
    }

    #[test]
    fn read_unknown_reason_errors_naming_path() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join(".wt");
        std::fs::create_dir_all(&wt).unwrap();
        let file = wt.join("UNK.removed.json");
        std::fs::write(
            &file,
            r#"{"id":"UNK","reason":"mystery","detail":null,"removed_at":1795000000}"#,
        )
        .unwrap();
        let err = read(dir.path(), "UNK").unwrap_err();
        assert!(
            err.contains(&file.display().to_string()),
            "expected error to contain path {}, got: {err}",
            file.display()
        );
        assert!(err.contains("mystery"), "got: {err}");
    }
}
