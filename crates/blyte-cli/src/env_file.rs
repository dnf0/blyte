//! Provider keys from a file, for sessions that did not export them: a Claude Code session started
//! without `GEMINI_API_KEY` failed every `blyte dispatch` at turn 0 (2026-09-29).
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

/// Parses a single `NAME=value` line (with optional `export ` prefix and surrounding quotes).
fn parse_line(line: &str) -> Option<(&str, &str)> {
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
    Some((name, value))
}

/// The `NAME=value` pairs in `text`.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = parse_line(line)?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

/// Reads the value of `name` from `path`, or `Ok(None)` if the file or variable is absent.
pub fn get(path: &Path, name: &str) -> Result<Option<String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    Ok(parse(&text)
        .into_iter()
        .rfind(|(k, _)| k == name)
        .map(|(_, v)| v))
}

/// Sets one or more `NAME=value` lines in the file at `path`: replaces an existing line for the
/// same name, keeps all other lines and comments, and appends new names. Creates the file and its
/// parent directory if needed, with file mode 0600 on Unix.
pub fn set(path: &Path, vars: &[(&str, &str)]) -> Result<(), String> {
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };

    let mut placed = vec![false; vars.len()];
    let mut out = String::new();
    for line in existing.lines() {
        if let Some((line_name, _)) = parse_line(line) {
            if let Some(idx) = vars.iter().position(|(k, _)| *k == line_name) {
                if !placed[idx] {
                    let (k, v) = vars[idx];
                    out.push_str(&format!("{k}={v}\n"));
                    placed[idx] = true;
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    for (idx, (k, v)) in vars.iter().enumerate() {
        if !placed[idx] {
            out.push_str(&format!("{k}={v}\n"));
            placed[idx] = true;
        }
    }

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(out.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    Ok(())
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
