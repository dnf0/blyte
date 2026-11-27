use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::serve::Surface;

const MAX_FINAL_MESSAGE_CHARS: usize = 4000;
const MAX_GATE_FAILURE_LINES: usize = 20;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

struct StartInfo {
    id: String,
    request: Option<String>,
    surface: Option<String>,
    started_at: u64,
    is_research: bool,
}

struct ParsedSummary {
    outcome: String,
    turns: u32,
    cost_usd: Option<f64>,
    seconds: u64,
    final_message: Option<String>,
    gate_failures: Vec<String>,
}

#[derive(Serialize)]
struct ReportedMarker<'a> {
    outcome: &'a str,
    ts: u64,
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

fn parse_start_file(wt_dir: &Path, path: &Path) -> Result<StartInfo, String> {
    let file_id = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".start.json"))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("invalid start file name: {}", path.display()))?;
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("run {file_id}: failed to read {}: {e}", path.display()))?;
    let val: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("run {file_id}: failed to parse {}: {e}", path.display()))?;
    if let Some(id_val) = val.get("id") {
        let field_id = id_val.as_str().ok_or_else(|| {
            format!(
                "run {file_id}: start file {} has non-string id",
                path.display()
            )
        })?;
        if field_id != file_id {
            return Err(format!(
                "run {file_id}: start file {} has id {field_id} which differs from file name id {file_id}",
                path.display()
            ));
        }
    }
    let request = match val.get("request") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(serde_json::Value::String(_)) => None,
        Some(_) => {
            return Err(format!(
                "run {file_id}: start file {} has non-string request field",
                path.display()
            ));
        }
    };
    let surface = match val.get("surface") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(format!(
                "run {file_id}: start file {} has non-string surface field",
                path.display()
            ));
        }
    };
    let is_research = is_research_start(wt_dir, file_id, &val);
    let started_at = match val.get("started_at") {
        Some(v) => v.as_u64().ok_or_else(|| {
            format!(
                "run {file_id}: start file {} has non-numeric started_at",
                path.display()
            )
        })?,
        None => 0,
    };
    Ok(StartInfo {
        id: file_id.to_string(),
        request,
        surface,
        started_at,
        is_research,
    })
}

fn parse_summary_file(run_id: &str, path: &Path, raw: &str) -> Result<ParsedSummary, String> {
    let val: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("run {run_id}: failed to parse {}: {e}", path.display()))?;
    let obj = val.as_object().ok_or_else(|| {
        format!(
            "run {run_id}: summary file {} is not a JSON object",
            path.display()
        )
    })?;

    let outcome = match obj.get("outcome").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            return Err(format!(
                "run {run_id}: summary file {} is missing outcome",
                path.display()
            ));
        }
    };

    let turns_u64 = obj.get("turns").and_then(|v| v.as_u64()).ok_or_else(|| {
        format!(
            "run {run_id}: summary file {} is missing numeric turns",
            path.display()
        )
    })?;
    let turns = u32::try_from(turns_u64).map_err(|_| {
        format!(
            "run {run_id}: summary file {} has out-of-range turns",
            path.display()
        )
    })?;

    let cost_usd = match obj.get("cost_usd") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_f64() {
            Some(c) if c.is_finite() => Some(c),
            _ => {
                return Err(format!(
                    "run {run_id}: summary file {} has invalid cost_usd",
                    path.display()
                ));
            }
        },
    };

    let seconds = obj.get("seconds").and_then(|v| v.as_u64()).ok_or_else(|| {
        format!(
            "run {run_id}: summary file {} is missing numeric seconds",
            path.display()
        )
    })?;

    let final_message = match obj.get("final_message") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(format!(
                "run {run_id}: summary file {} has non-string final_message",
                path.display()
            ));
        }
    };

    let gate_failures = match obj.get("gate_failures") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let s = item.as_str().ok_or_else(|| {
                    format!(
                        "run {run_id}: summary file {} has non-string entry in gate_failures",
                        path.display()
                    )
                })?;
                out.push(s.to_string());
            }
            out
        }
        Some(_) => {
            return Err(format!(
                "run {run_id}: summary file {} has non-array gate_failures",
                path.display()
            ));
        }
    };

    Ok(ParsedSummary {
        outcome,
        turns,
        cost_usd,
        seconds,
        final_message,
        gate_failures,
    })
}

fn format_first_line(run_id: &str, summary: &ParsedSummary) -> String {
    let minutes = summary.seconds as f64 / 60.0;
    let mut parts = Vec::with_capacity(4);
    parts.push(format!("Run {run_id}: {}", summary.outcome));
    parts.push(format!("{} turns", summary.turns));
    if let Some(cost) = summary.cost_usd {
        parts.push(format!("USD {cost:.2}"));
    }
    parts.push(format!("{minutes:.1} min"));
    parts.join(", ")
}

fn truncate_final_message(run_id: &str, message: &str) -> String {
    if let Some((byte_idx, _)) = message.char_indices().nth(MAX_FINAL_MESSAGE_CHARS) {
        format!(
            "{}\n… (the rest: blyte review {run_id})",
            &message[..byte_idx]
        )
    } else {
        message.to_string()
    }
}

fn format_notice_text(run_id: &str, summary: &ParsedSummary) -> String {
    let first_line = format_first_line(run_id, summary);
    match summary.final_message.as_deref() {
        Some(msg) if !msg.is_empty() => {
            format!("{first_line}\n{}", truncate_final_message(run_id, msg))
        }
        _ => first_line,
    }
}

fn last_gate_failure_lines(gate_failures: &[String]) -> Option<String> {
    let lines: Vec<&str> = gate_failures.iter().flat_map(|s| s.lines()).collect();
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(MAX_GATE_FAILURE_LINES);
    Some(lines[start..].join("\n"))
}

fn format_failure_queue_message(run_id: &str, summary: &ParsedSummary) -> String {
    let header = format!("Run {run_id} did not pass: {}.", summary.outcome);
    let footer = format!("Read blyte review {run_id} and tell the user, or dispatch it again.");
    match last_gate_failure_lines(&summary.gate_failures) {
        Some(tail) => format!("{header}\n{tail}\n{footer}"),
        None => format!("{header}\n{footer}"),
    }
}

fn format_research_queue_message(
    run_id: &str,
    summary: &ParsedSummary,
    records_dir: &Path,
    in_place: bool,
) -> String {
    if in_place {
        if matches!(summary.outcome.as_str(), "verified" | "finished") {
            format!(
                "Research run {run_id} finished: {}. Report: {}",
                summary.outcome,
                records_dir.join(format!("{run_id}.report.md")).display()
            )
        } else {
            format!(
                "Research run {run_id} finished: {}. Log: {}",
                summary.outcome,
                records_dir.join(format!("{run_id}.log")).display()
            )
        }
    } else if summary.outcome == "verified" {
        format!("Research run {run_id} finished: verified. Report: .wt/{run_id}.report.md")
    } else {
        format!(
            "Research run {run_id} finished: {}. Log: .wt/{run_id}.log",
            summary.outcome
        )
    }
}

fn now_unix_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| format!("system clock before unix epoch: {e}"))
}

fn write_marker_atomic(
    run_id: &str,
    path: &Path,
    marker: &ReportedMarker<'_>,
) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(marker).map_err(|e| {
        format!(
            "run {run_id}: failed to serialize marker {}: {e}",
            path.display()
        )
    })?;
    json.push('\n');
    let mut tmp_name = path.as_os_str().to_os_string();
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    tmp_name.push(format!(".tmp.{}.{seq}", std::process::id()));
    let tmp_path = PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, json).map_err(|e| {
        format!(
            "run {run_id}: failed to write marker {} via {}: {e}",
            path.display(),
            tmp_path.display()
        )
    })?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!(
            "run {run_id}: failed to write marker {}: {e}",
            path.display()
        )
    })
}

fn report_single_run(
    surface: &Surface,
    wt_dir: &Path,
    run_id: &str,
    request: &str,
    is_research: bool,
    in_place: bool,
) -> Result<(), String> {
    let marker_path = wt_dir.join(format!("{run_id}.reported.json"));
    if marker_path.exists() {
        return Ok(());
    }

    let summary_path = wt_dir.join(format!("{run_id}.json"));
    let raw = match std::fs::read_to_string(&summary_path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "run {run_id}: failed to read {}: {e}",
                summary_path.display()
            ));
        }
    };

    let summary = parse_summary_file(run_id, &summary_path, &raw)?;
    let notice = format_notice_text(run_id, &summary);

    surface
        .session
        .record_decision(run_id, "finished", Some(&notice), None)
        .map_err(|e| {
            format!(
                "run {run_id}: failed to record session entry for {}: {e}",
                summary_path.display()
            )
        })?;

    let passed = if in_place {
        matches!(summary.outcome.as_str(), "verified" | "finished")
    } else {
        summary.outcome == "verified"
    };

    let queued_message = if is_research {
        Some(format_research_queue_message(
            run_id, &summary, wt_dir, in_place,
        ))
    } else if !passed {
        Some(format_failure_queue_message(run_id, &summary))
    } else {
        None
    };

    if let Some(msg) = queued_message {
        surface
            .session
            .queue_for_request(request, &msg)
            .map_err(|e| {
                format!(
                    "run {run_id}: failed to queue agent message for {}: {e}",
                    summary_path.display()
                )
            })?;
    }

    let ts = now_unix_secs().map_err(|e| {
        format!(
            "run {run_id}: failed to write {}: {e}",
            marker_path.display()
        )
    })?;
    let marker = ReportedMarker {
        outcome: &summary.outcome,
        ts,
    };
    write_marker_atomic(run_id, &marker_path, &marker)
}

pub fn report_finished_runs(surface: &Surface) -> Result<(), String> {
    let wt_dir = surface.root.join(".wt");
    report_finished_runs_impl(surface, &wt_dir, false)
}

pub fn report_finished_runs_in(surface: &Surface, records_dir: &Path) -> Result<(), String> {
    report_finished_runs_impl(surface, records_dir, true)
}

fn report_finished_runs_impl(
    surface: &Surface,
    records_dir: &Path,
    in_place: bool,
) -> Result<(), String> {
    let start_files = list_start_files(records_dir)?;
    let mut candidates = Vec::new();
    let mut errors = Vec::new();

    for path in start_files {
        match parse_start_file(records_dir, &path) {
            Ok(info) => {
                if info.surface.as_deref() == Some(surface.surface_id.as_str()) {
                    if let Some(request) = info.request {
                        candidates.push((info.started_at, info.id, request, info.is_research));
                    }
                }
            }
            Err(e) => errors.push(e),
        }
    }

    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    for (_, run_id, request, is_research) in candidates {
        if let Err(e) = report_single_run(
            surface,
            records_dir,
            &run_id,
            &request,
            is_research,
            in_place,
        ) {
            errors.push(e);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
