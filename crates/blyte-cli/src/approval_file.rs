use std::ffi::OsStr;
use std::path::PathBuf;

pub fn path_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(dir) = xdg.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("blyte").join("allow-unsandboxed"));
    }
    home.filter(|v| !v.is_empty())
        .map(|h| PathBuf::from(h).join(".config/blyte/allow-unsandboxed"))
}

pub fn load() -> Result<Vec<String>, String> {
    let Some(path) = path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    ) else {
        return Ok(Vec::new());
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect())
}
