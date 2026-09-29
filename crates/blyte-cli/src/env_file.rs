//! Provider keys from a file, for sessions that did not export them: a Claude Code session started
//! without `GEMINI_API_KEY` failed every `blyte dispatch` at turn 0 (spatial-service, 2026-09-29).
//!
//! The file is `$BLYTE_ENV_FILE` (empty turns it off), else `$XDG_CONFIG_HOME/blyte/env`, else
//! `~/.config/blyte/env`: `NAME=value` lines, with an optional `export ` prefix, surrounding quotes,
//! blank lines and `#` comments. A variable already set in the environment wins. A file other users
//! can read is not loaded.

use std::path::{Path, PathBuf};

/// Where blyte looks for the file, or `None` when it is turned off.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BLYTE_ENV_FILE") {
        return if p.is_empty() {
            None
        } else {
            Some(PathBuf::from(p))
        };
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x).join("blyte/env"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/blyte/env"))
}

/// The `NAME=value` pairs in `text`.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

/// Sets each variable in the file at `path` that is not already set, and returns their names.
/// A missing file sets nothing. Call it before any threads start: it uses `std::env::set_var`.
pub fn load(path: &Path) -> Result<Vec<String>, String> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(Vec::new());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{} can be read by other users, so it was not loaded; run: chmod 600 {}",
                path.display(),
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut set = Vec::new();
    for (name, value) in parse(&text) {
        if std::env::var_os(&name).is_none() {
            std::env::set_var(&name, value);
            set.push(name);
        }
    }
    Ok(set)
}
