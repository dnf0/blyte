use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{registry, update};

const DAY_SECS: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    pub checked_at: u64,
    pub tag: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
}

pub fn summarize(body: &str) -> String {
    let bullets: Vec<String> = body
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(|b| b.replace("**", ""))
        .collect();
    if bullets.is_empty() {
        return String::new();
    }
    let joined = bullets.join("; ");
    if joined.len() > 200 {
        let mut end = 200;
        while !joined.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &joined[..end])
    } else {
        joined
    }
}

pub fn notice_line(current: &str, tag: &str, summary: &str) -> Option<String> {
    let current_ver = update::parse_version(current)?;
    let tag_ver = update::parse_version(tag)?;
    if tag_ver <= current_ver {
        return None;
    }
    let ver = tag.trim().strip_prefix('v').unwrap_or(tag.trim());
    if summary.is_empty() {
        Some(format!(
            "blyte: {ver} is out (you have {current}). Run `blyte update`."
        ))
    } else {
        Some(format!(
            "blyte: {ver} is out (you have {current}): {summary}. Run `blyte update`."
        ))
    }
}

pub fn check(
    state_dir: &Path,
    current: &str,
    now: u64,
    fetch: impl FnOnce() -> Result<(String, String), String>,
) -> Vec<String> {
    let path = state_dir.join("latest-release.json");
    let cached: Option<Cache> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    if let Some(c) = &cached {
        if now.saturating_sub(c.checked_at) < DAY_SECS {
            let mut lines = Vec::new();
            if let Some(tag) = c.tag.as_deref() {
                let summary = c.summary.as_deref().unwrap_or("");
                if let Some(line) = notice_line(current, tag, summary) {
                    lines.push(line);
                }
            }
            return lines;
        }
    }

    let prev_tag = cached.as_ref().and_then(|c| c.tag.clone());
    let prev_summary = cached.as_ref().and_then(|c| c.summary.clone());

    let fetched = fetch().and_then(|(tag, body)| {
        if update::parse_version(&tag).is_none() {
            Err(format!("unexpected tag {tag}"))
        } else {
            Ok((tag, body))
        }
    });

    let mut lines = Vec::new();
    let new_cache = match fetched {
        Ok((tag, body)) => {
            let summary = summarize(&body);
            if let Some(line) = notice_line(current, &tag, &summary) {
                lines.push(line);
            }
            Cache {
                checked_at: now,
                tag: Some(tag),
                summary: Some(summary),
                error: None,
            }
        }
        Err(e) => {
            lines.push(format!("blyte: could not check for a newer release: {e}"));
            if let Some(tag) = prev_tag.as_deref() {
                let summary = prev_summary.as_deref().unwrap_or("");
                if let Some(line) = notice_line(current, tag, summary) {
                    lines.push(line);
                }
            }
            Cache {
                checked_at: now,
                tag: prev_tag,
                summary: prev_summary,
                error: Some(e),
            }
        }
    };

    if let Err(e) = write_cache(state_dir, &path, &new_cache) {
        lines.push(format!(
            "blyte: could not save the release check to {}: {e}",
            path.display()
        ));
    }

    lines
}

fn write_cache(state_dir: &Path, path: &Path, cache: &Cache) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(cache).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

pub fn print_notice() {
    if std::env::var_os("BLYTE_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty()) {
        return;
    }
    let Some(state_dir) = registry::state_dir() else {
        eprintln!(
            "blyte: could not check for a newer release: no state directory (set HOME or BLYTE_STATE_DIR)"
        );
        return;
    };
    let version = env!("CARGO_PKG_VERSION");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let lines = check(&state_dir, version, now, || fetch_latest_release(version));
    for line in lines {
        eprintln!("{line}");
    }
}

fn fetch_latest_release(version: &str) -> Result<(String, String), String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        update::REPO
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build();
    let resp = match agent
        .get(&url)
        .set("User-Agent", &format!("blyte/{version}"))
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
        Err(ureq::Error::Transport(t)) => return Err(t.to_string()),
    };
    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let tag_name = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing tag_name".to_string())?
        .to_string();
    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((tag_name, body))
}
