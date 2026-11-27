use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use crate::session::{Entry, EntryBody};
use crate::surface_dir::{is_running, list_records_for_root};
use crate::surface_registry::{find_by_name, first_line, live_entries, RegistryEntry};

/// A stopped surface belonging to a folder.
///
/// `last_used` is in Unix seconds: the `ts` of the last complete entry in
/// `entries.jsonl`, or the record's `started_at` when that file is absent or
/// has no entries (a surface that was started and never used was last used
/// when it started).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoppedSurface {
    pub id: String,
    pub name: String,
    pub last_used: u64,
    pub last_message: Option<String>,
}

fn scan_entries_summary(path: &Path) -> Result<(Option<u64>, Option<String>), String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, None)),
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };

    let mut last_ts: Option<u64> = None;
    let mut last_user_text: Option<String> = None;
    let mut pos = 0usize;
    let mut line_no = 0usize;
    while pos < bytes.len() {
        line_no += 1;
        let start = pos;
        let end = match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(rel) => pos + rel + 1,
            None => bytes.len(),
        };
        let is_last_without_newline = end == bytes.len() && !bytes[start..end].ends_with(b"\n");
        let line_slice = &bytes[start..end];
        let parsed = std::str::from_utf8(line_slice)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Entry>(s).map_err(|e| e.to_string()));
        match parsed {
            Ok(entry) => {
                last_ts = Some(entry.ts);
                if let EntryBody::UserMessage { text, .. } = entry.body {
                    last_user_text = Some(text);
                }
            }
            Err(reason) => {
                if !is_last_without_newline {
                    return Err(format!(
                        "invalid entry in {} at line {}: {}",
                        path.display(),
                        line_no,
                        reason
                    ));
                }
            }
        }
        pos = end;
    }

    let last_message = last_user_text.and_then(|text| {
        let line = first_line(&text);
        if line.is_empty() {
            None
        } else {
            Some(line)
        }
    });

    Ok((last_ts, last_message))
}

fn stopped_surfaces_for_root(root: &Path, state_dir: &Path) -> Result<Vec<StoppedSurface>, String> {
    let records = list_records_for_root(root, Some(state_dir))?;
    let mut stopped = Vec::new();
    for located in records {
        if is_running(&located.record) {
            continue;
        }
        let entries_path = located.dir.join("entries.jsonl");
        let (last_ts, last_message) = scan_entries_summary(&entries_path)?;
        let last_used = last_ts.unwrap_or(located.record.started_at);
        stopped.push(StoppedSurface {
            id: located.record.id,
            name: located.record.name,
            last_used,
            last_message,
        });
    }
    stopped.sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| a.id.cmp(&b.id)));
    Ok(stopped)
}

/// Collect stopped surfaces belonging to `folder`, newest first by `last_used`, then by `id`.
pub fn stopped_surfaces(folder: &Path, state_dir: &Path) -> Result<Vec<StoppedSurface>, String> {
    let root = crate::serve::resolve_folder(folder)?;
    stopped_surfaces_for_root(&root, state_dir)
}

fn format_utc_minute(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let secs_of_day = ts % 86_400;
    let hour = secs_of_day / 3_600;
    let min = (secs_of_day % 3_600) / 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02} {hour:02}:{min:02}")
}

pub fn format_stopped_surfaces_table(entries: &[StoppedSurface], folder: &Path) -> String {
    if entries.is_empty() {
        return format!("no stopped surfaces in {}\n", folder.display());
    }

    let rows: Vec<(&str, &str, String, String)> = entries
        .iter()
        .map(|entry| {
            let last_used = format_utc_minute(entry.last_used);
            let message = match &entry.last_message {
                Some(msg) => msg.chars().take(60).collect(),
                None => "-".to_string(),
            };
            (entry.name.as_str(), entry.id.as_str(), last_used, message)
        })
        .collect();

    let name_w = rows
        .iter()
        .map(|row| row.0.chars().count())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    let id_w = rows
        .iter()
        .map(|row| row.1.chars().count())
        .max()
        .unwrap_or(0)
        .max("ID".len());
    let used_w = rows
        .iter()
        .map(|row| row.2.chars().count())
        .max()
        .unwrap_or(0)
        .max("LAST USED".len());

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<name_w$}  {:<id_w$}  {:<used_w$}  LAST MESSAGE",
        "NAME", "ID", "LAST USED"
    );
    for (name, id, last_used, message) in rows {
        let _ = writeln!(
            out,
            "{name:<name_w$}  {id:<id_w$}  {last_used:<used_w$}  {message}"
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "resume with: blyte --resume <name or id>");
    out
}

pub fn run_stopped_surfaces(
    folder: &Path,
    state_dir: &Path,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let root = match crate::serve::resolve_folder(folder) {
        Ok(root) => root,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };
    let entries = match stopped_surfaces_for_root(&root, state_dir) {
        Ok(entries) => entries,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    if json {
        match serde_json::to_string_pretty(&entries) {
            Ok(text) => {
                let _ = writeln!(out, "{text}");
                return 0;
            }
            Err(e) => {
                let _ = writeln!(err, "blyte: failed to serialize surfaces: {e}");
                return 1;
            }
        }
    }

    let _ = write!(out, "{}", format_stopped_surfaces_table(&entries, &root));
    0
}

fn repository_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

pub fn format_surfaces_table(entries: &[RegistryEntry], current_id: Option<&str>) -> String {
    if entries.is_empty() {
        return "no surfaces are running\n".to_string();
    }

    let rows: Vec<(String, String, &str, &str)> = entries
        .iter()
        .map(|entry| {
            let name = if current_id == Some(entry.id.as_str()) {
                format!("{} (this surface)", entry.name)
            } else {
                entry.name.clone()
            };
            (
                name,
                repository_name(&entry.root),
                entry.branch.as_deref().unwrap_or("-"),
                entry.doing.as_str(),
            )
        })
        .collect();

    let name_w = rows
        .iter()
        .map(|row| row.0.chars().count())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    let repo_w = rows
        .iter()
        .map(|row| row.1.chars().count())
        .max()
        .unwrap_or(0)
        .max("REPOSITORY".len());
    let branch_w = rows
        .iter()
        .map(|row| row.2.chars().count())
        .max()
        .unwrap_or(0)
        .max("BRANCH".len());

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<name_w$}  {:<repo_w$}  {:<branch_w$}  DOING",
        "NAME", "REPOSITORY", "BRANCH"
    );
    for (name, repo, branch, doing) in rows {
        let _ = writeln!(
            out,
            "{name:<name_w$}  {repo:<repo_w$}  {branch:<branch_w$}  {doing}"
        );
    }
    out
}

pub fn run_surfaces(dir: &Path, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let entries = match live_entries(dir) {
        Ok(entries) => entries,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    if json {
        match serde_json::to_string_pretty(&entries) {
            Ok(text) => {
                let _ = writeln!(out, "{text}");
                return 0;
            }
            Err(e) => {
                let _ = writeln!(err, "blyte: failed to serialize surfaces: {e}");
                return 1;
            }
        }
    }

    let _ = write!(out, "{}", format_surfaces_table(&entries, None));
    0
}

pub fn surface_url(entry: &RegistryEntry) -> Result<String, String> {
    let token_path = blyte_agent::sandbox::surface_token_path_for_id(&entry.id)?;
    let raw_token = std::fs::read_to_string(&token_path).map_err(|_| {
        format!(
            "surface {} has no usable token at {}",
            entry.name,
            token_path.display()
        )
    })?;
    let token = raw_token.trim();
    if token.is_empty() || token.contains(['\n', '\r']) {
        return Err(format!(
            "surface {} has no usable token at {}",
            entry.name,
            token_path.display()
        ));
    }
    Ok(format!("http://127.0.0.1:{}/#token={token}", entry.port))
}

pub fn surfaces_payload(
    registry_dir: &Path,
    current_id: &str,
    stopped: &[StoppedSurface],
) -> Result<serde_json::Value, String> {
    let mut entries = live_entries(registry_dir)?;
    entries.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.started_at.cmp(&b.started_at))
    });
    if !entries.iter().any(|entry| entry.id == current_id) {
        return Err(format!(
            "current surface {current_id} is not in the surface registry"
        ));
    }
    let surfaces: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let (url, error) = match surface_url(entry) {
                Ok(url) => (Some(url), None),
                Err(err) => (None, Some(err)),
            };
            serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "root": entry.root.to_string_lossy(),
                "branch": entry.branch,
                "doing": entry.doing,
                "current": entry.id == current_id,
                "url": url,
                "error": error,
            })
        })
        .collect();
    Ok(serde_json::json!({ "surfaces": surfaces, "stopped": stopped }))
}

pub fn run_open(
    dir: &Path,
    name: &str,
    opener: &dyn Fn(&str),
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let entry = match find_by_name(dir, name) {
        Ok(entry) => entry,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    let url = match surface_url(&entry) {
        Ok(url) => url,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    opener(&url);
    let _ = writeln!(out, "blyte: opened surface {}", entry.name);
    0
}
