use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use blyte_agent::sandbox::validate_surface_id;
use serde::{Deserialize, Serialize};

const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRecord {
    pub id: String,
    pub name: String,
    pub pid: u32,
    pub port: u16,
    pub session_id: String,
    pub started_at: u64,
}

pub fn new_surface_id() -> Result<String, String> {
    let mut bytes = [0u8; 8];
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|e| format!("failed to open /dev/urandom: {e}"))?;
    file.read_exact(&mut bytes)
        .map_err(|e| format!("failed to read /dev/urandom: {e}"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(17);
    out.push('s');
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

pub fn validate_surface_name(name: &str) -> Result<(), String> {
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid surface name: {name} (use letters, digits, '.', '_' and '-', starting with a letter or digit)"
        ))
    }
}

pub fn surfaces_dir(main_checkout: &Path) -> PathBuf {
    main_checkout.join(".wt").join("surfaces")
}

pub fn surface_dir(main_checkout: &Path, id: &str) -> PathBuf {
    surfaces_dir(main_checkout).join(id)
}

pub fn record_path(main_checkout: &Path, id: &str) -> PathBuf {
    surface_dir(main_checkout, id).join("surface.json")
}

pub fn entries_path(main_checkout: &Path, id: &str) -> PathBuf {
    surface_dir(main_checkout, id).join("entries.jsonl")
}

pub fn write_record(main_checkout: &Path, record: &SurfaceRecord) -> Result<(), String> {
    validate_surface_id(&record.id)?;
    validate_surface_name(&record.name)?;
    let dir = surface_dir(main_checkout, &record.id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let mut json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("failed to serialize surface record {}: {e}", record.id))?;
    json.push('\n');
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("surface.json.tmp.{}.{seq}", std::process::id()));
    let dest = record_path(main_checkout, &record.id);
    if let Err(e) = std::fs::write(&tmp, json.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("failed to write {}: {e}", tmp.display()));
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

pub fn read_record(main_checkout: &Path, id: &str) -> Result<SurfaceRecord, String> {
    validate_surface_id(id)?;
    let path = record_path(main_checkout, id);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let record: SurfaceRecord = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    if record.id != id {
        return Err(format!(
            "surface record {} has id {}, expected {id}",
            path.display(),
            record.id
        ));
    }
    if let Err(e) = validate_surface_name(&record.name) {
        return Err(format!("surface record {} has {e}", path.display()));
    }
    Ok(record)
}

pub fn list_records(main_checkout: &Path) -> Result<Vec<SurfaceRecord>, String> {
    let root = surfaces_dir(main_checkout);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("failed to read {}: {e}", root.display())),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read {}: {e}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("failed to inspect {}: {e}", path.display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let name_os = entry.file_name();
        let Some(id) = name_os.to_str() else {
            return Err(format!("invalid surface directory: {}", path.display()));
        };
        if let Err(e) = validate_surface_id(id) {
            return Err(format!("invalid surface directory {}: {e}", path.display()));
        }
        let record = read_record(main_checkout, id)?;
        records.push(record);
    }
    records.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(records)
}

pub fn is_running(record: &SurfaceRecord) -> bool {
    if record.port == 0 {
        return false;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], record.port));
    let mut stream = match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(_) => return false,
    };
    if stream.set_read_timeout(Some(PROBE_TIMEOUT)).is_err() {
        return false;
    }
    if stream.set_write_timeout(Some(PROBE_TIMEOUT)).is_err() {
        return false;
    }
    let req = format!(
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        record.port
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut raw = Vec::new();
    if stream.read_to_end(&mut raw).is_err() {
        return false;
    }
    let text = match std::str::from_utf8(&raw) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some(parts) => parts,
        None => return false,
    };
    let status_line = head.lines().next().unwrap_or("");
    if !(status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200")) {
        return false;
    }
    let val: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    val.get("surface_id").and_then(|v| v.as_str()) == Some(record.id.as_str())
}

pub fn pick_name(
    default_base: &str,
    requested: Option<&str>,
    running_names: &[String],
) -> Result<String, String> {
    if let Some(name) = requested {
        validate_surface_name(name)?;
        if running_names.iter().any(|r| r == name) {
            return Err(format!(
                "a running surface is already named {name}; pass a different --name"
            ));
        }
        return Ok(name.to_string());
    }
    if validate_surface_name(default_base).is_err() {
        return Err(format!(
            "the repository directory name {default_base} cannot be a surface name; pass --name"
        ));
    }
    if !running_names.iter().any(|r| r == default_base) {
        return Ok(default_base.to_string());
    }
    for suffix in 2u64.. {
        let candidate = format!("{default_base}-{suffix}");
        if !running_names.iter().any(|r| r == &candidate) {
            validate_surface_name(&candidate)?;
            return Ok(candidate);
        }
    }
    unreachable!()
}
