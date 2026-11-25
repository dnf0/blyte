use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use blyte_agent::sandbox::validate_surface_id;
use serde::{Deserialize, Serialize};

use crate::surface_dir::{health_matches, validate_surface_name};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub branch: String,
    pub pid: u32,
    pub port: u16,
    pub started_at: u64,
    pub doing: String,
}

pub fn registry_dir() -> Result<PathBuf, String> {
    crate::registry::state_dir()
        .map(|dir| dir.join("surfaces"))
        .ok_or_else(|| {
            "cannot find the blyte state directory: set BLYTE_STATE_DIR, XDG_STATE_HOME or HOME"
                .to_string()
        })
}

pub fn entry_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

pub fn write_entry(dir: &Path, entry: &RegistryEntry) -> Result<(), String> {
    validate_surface_id(&entry.id)?;
    validate_surface_name(&entry.name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let dest = entry_path(dir, &entry.id);
    let mut json = serde_json::to_string_pretty(entry)
        .map_err(|e| format!("failed to serialize {}: {e}", dest.display()))?;
    json.push('\n');
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{}.tmp.{}.{seq}", entry.id, std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&tmp)
        .map_err(|e| format!("failed to create {}: {e}", tmp.display()))?;
    if let Err(e) = file.write_all(json.as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("failed to write {}: {e}", tmp.display()));
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "failed to set permissions on {}: {e}",
                tmp.display()
            ));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "failed to rename {} to {}: {e}",
            tmp.display(),
            dest.display()
        ));
    }
    Ok(())
}

pub fn remove_entry(dir: &Path, id: &str) -> Result<(), String> {
    let path = entry_path(dir, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("failed to remove {}: {e}", path.display())),
    }
}

pub fn read_entries(dir: &Path) -> Result<Vec<RegistryEntry>, String> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("failed to read {}: {e}", dir.display())),
    };
    let mut entries = Vec::new();
    for item in read_dir {
        let item = item.map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
        let path = item.path();
        let file_name = item.file_name();
        let lossy = file_name.to_string_lossy();
        if !lossy.ends_with(".json") {
            continue;
        }
        let Some(name_str) = file_name.to_str() else {
            return Err(format!("invalid registry entry path: {}", path.display()));
        };
        let Some(stem) = name_str.strip_suffix(".json") else {
            continue;
        };
        // A surface removes its entry when it exits, which can happen between the listing above
        // and this read: that surface is no longer running, so it is not in the result.
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
        };
        let entry: RegistryEntry = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        if entry.id != stem {
            return Err(format!(
                "surface registry entry {} has id {}, expected {stem}",
                path.display(),
                entry.id
            ));
        }
        entries.push(entry);
    }
    entries.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(entries)
}

pub fn live_entries(dir: &Path) -> Result<Vec<RegistryEntry>, String> {
    let entries = read_entries(dir)?;
    let mut live = Vec::with_capacity(entries.len());
    for entry in entries {
        if health_matches(entry.port, &entry.id) {
            live.push(entry);
        } else {
            remove_entry(dir, &entry.id)?;
        }
    }
    Ok(live)
}

pub fn find_by_name(dir: &Path, name: &str) -> Result<RegistryEntry, String> {
    let entries = live_entries(dir)?;
    let mut matches: Vec<RegistryEntry> = entries
        .into_iter()
        .filter(|entry| entry.name == name || entry.id == name)
        .collect();
    match matches.len() {
        0 => Err(format!("no running surface named {name}")),
        1 => Ok(matches.remove(0)),
        _ => {
            let ids: Vec<&str> = matches.iter().map(|e| e.id.as_str()).collect();
            Err(format!(
                "more than one running surface is named {name}; use an id: {}",
                ids.join(", ")
            ))
        }
    }
}

pub fn set_doing(dir: &Path, id: &str, doing: &str) -> Result<(), String> {
    let path = entry_path(dir, id);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut entry: RegistryEntry = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    if entry.id != id {
        return Err(format!(
            "surface registry entry {} has id {}, expected {id}",
            path.display(),
            entry.id
        ));
    }
    entry.doing = doing.to_string();
    write_entry(dir, &entry)
}

pub fn first_line(text: &str) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.chars().take(120).collect();
        }
    }
    String::new()
}
