use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use blyte_agent::sandbox::{is_git_repository, validate_surface_id};
use serde::{Deserialize, Serialize};

const PROBE_TIMEOUT: Duration = Duration::from_millis(300);
const PROBE_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
const STARTING_MARKER: &str = ".starting";

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Running,
    NotRunning,
    Unknown,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedRecord {
    pub record: SurfaceRecord,
    pub dir: PathBuf,
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::borrow::Cow::Borrowed(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::borrow::Cow::Owned(path.to_string_lossy().into_owned().into_bytes())
    }
}

fn folder_store_key(canonical: &Path) -> String {
    format!("{:016x}", fnv1a_64(&path_bytes(canonical)))
}

fn ensure_dir_0700(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let meta = std::fs::metadata(dir)
                    .map_err(|me| format!("failed to inspect {}: {me}", dir.display()))?;
                if !meta.is_dir() {
                    return Err(format!(
                        "folder store {} exists and is not a directory",
                        dir.display()
                    ));
                }
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(
                    |pe| format!("failed to set permissions on {}: {pe}", dir.display()),
                )?;
            }
            Err(e) => return Err(format!("failed to create {}: {e}", dir.display())),
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))
    }
}

fn verify_root_file(store_dir: &Path, canonical: &Path) -> Result<bool, String> {
    let root_file = store_dir.join("root");
    let raw = match std::fs::read(&root_file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("failed to read {}: {e}", root_file.display())),
    };
    let stored = raw.strip_suffix(b"\n").unwrap_or(&raw);
    let stored = stored.strip_suffix(b"\r").unwrap_or(stored);
    let expected = path_bytes(canonical);
    if stored != expected.as_ref() {
        let stored_display = String::from_utf8_lossy(stored);
        return Err(format!(
            "folder store collision in {}: recorded root {} differs from {}",
            store_dir.display(),
            stored_display,
            canonical.display()
        ));
    }
    Ok(true)
}

pub fn folder_store(state_dir: &Path, root: &Path) -> Result<PathBuf, String> {
    let canonical = root
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize {}: {e}", root.display()))?;
    let key = folder_store_key(&canonical);
    let store_dir = state_dir.join("folders").join(key);
    ensure_dir_0700(&store_dir)?;
    if !verify_root_file(&store_dir, &canonical)? {
        let root_file = store_dir.join("root");
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = store_dir.join(format!(".root.tmp.{}.{seq}", std::process::id()));
        let content = path_bytes(&canonical);
        if let Err(e) = std::fs::write(&tmp, content.as_ref()) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("failed to write {}: {e}", tmp.display()));
        }
        if let Err(e) = std::fs::rename(&tmp, &root_file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "failed to rename {} to {}: {e}",
                tmp.display(),
                root_file.display()
            ));
        }
    }
    Ok(store_dir)
}

pub fn run_store(state_dir: &Path, root: &Path) -> Result<PathBuf, String> {
    if is_git_repository(root) {
        return Ok(root.join(".wt"));
    }
    let dir = folder_store(state_dir, root)?.join("runs");
    ensure_dir_0700(&dir)?;
    Ok(dir)
}

pub fn tasks_dir(state_dir: &Path, root: &Path) -> Result<PathBuf, String> {
    if is_git_repository(root) {
        return Ok(root.join(".wt").join("tasks"));
    }
    let dir = folder_store(state_dir, root)?.join("tasks");
    ensure_dir_0700(&dir)?;
    Ok(dir)
}

pub fn in_place_lock_path(state_dir: &Path, root: &Path) -> Result<PathBuf, String> {
    if is_git_repository(root) {
        return Err(format!(
            "in-place runs are for folders that are not git repositories: {}",
            root.display()
        ));
    }
    Ok(folder_store(state_dir, root)?.join("in-place.lock"))
}

pub(crate) fn existing_folder_store(
    state_dir: &Path,
    root: &Path,
) -> Result<Option<PathBuf>, String> {
    let canonical = match root.canonicalize() {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("failed to canonicalize {}: {e}", root.display())),
    };
    let key = folder_store_key(&canonical);
    let store_dir = state_dir.join("folders").join(key);
    match std::fs::metadata(&store_dir) {
        Ok(meta) if meta.is_dir() => {
            let _ = verify_root_file(&store_dir, &canonical)?;
            Ok(Some(store_dir))
        }
        Ok(_) => Err(format!(
            "folder store {} exists and is not a directory",
            store_dir.display()
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("failed to inspect {}: {e}", store_dir.display())),
    }
}

pub fn surfaces_dir(main_checkout: &Path) -> PathBuf {
    main_checkout.join(".wt").join("surfaces")
}

pub fn surface_dir(main_checkout: &Path, id: &str) -> PathBuf {
    surfaces_dir(main_checkout).join(id)
}

pub fn prepare_starting_dir(dir: &Path) -> Result<(), String> {
    let parent = dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("surface directory has no parent: {}", dir.display()))?;
    let staging_parent = parent
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("surfaces directory has no parent: {}", parent.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let staging_dir =
        staging_parent.join(format!(".surface-starting.{}.{seq}", std::process::id()));
    std::fs::create_dir(&staging_dir)
        .map_err(|e| format!("failed to create {}: {e}", staging_dir.display()))?;
    let marker = staging_dir.join(STARTING_MARKER);
    if let Err(e) = std::fs::write(&marker, b"") {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(format!("failed to write {}: {e}", marker.display()));
    }
    if let Err(e) = std::fs::rename(&staging_dir, dir) {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(format!(
            "failed to rename {} to {}: {e}",
            staging_dir.display(),
            dir.display()
        ));
    }
    Ok(())
}

pub fn cleanup_starting_dir(dir: &Path) {
    if dir.join("surface.json").exists() {
        return;
    }
    if let Some(staging_parent) = dir
        .parent()
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty())
    {
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let trash = staging_parent.join(format!(".surface-cleanup.{}.{seq}", std::process::id()));
        if std::fs::rename(dir, &trash).is_ok() {
            let _ = std::fs::remove_dir_all(&trash);
            return;
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

pub fn write_record_in(dir: &Path, record: &SurfaceRecord) -> Result<(), String> {
    validate_surface_id(&record.id)?;
    validate_surface_name(&record.name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let mut json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("failed to serialize surface record {}: {e}", record.id))?;
    json.push('\n');
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("surface.json.tmp.{}.{seq}", std::process::id()));
    let dest = dir.join("surface.json");
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
    let _ = std::fs::remove_file(dir.join(STARTING_MARKER));
    Ok(())
}

fn parse_record_text(path: &Path, id: &str, text: &str) -> Result<SurfaceRecord, String> {
    let record: SurfaceRecord = serde_json::from_str(text)
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

pub fn read_record_in(dir: &Path, id: &str) -> Result<SurfaceRecord, String> {
    validate_surface_id(id)?;
    let path = dir.join("surface.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    parse_record_text(&path, id, &text)
}

pub fn read_record(main_checkout: &Path, id: &str) -> Result<SurfaceRecord, String> {
    read_record_in(&surface_dir(main_checkout, id), id)
}

pub fn list_records_in(root: &Path) -> Result<Vec<LocatedRecord>, String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("failed to read {}: {e}", root.display())),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read {}: {e}", root.display()))?;
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("failed to inspect {}: {e}", path.display())),
        };
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
        let record_path = path.join("surface.json");
        let record = match std::fs::read_to_string(&record_path) {
            Ok(text) => parse_record_text(&record_path, id, &text)?,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                if path.join(STARTING_MARKER).is_file() {
                    continue;
                }
                match std::fs::read_to_string(&record_path) {
                    Ok(text) => parse_record_text(&record_path, id, &text)?,
                    Err(e2) if e2.kind() == ErrorKind::NotFound && !path.exists() => continue,
                    Err(e2) => {
                        return Err(format!("failed to read {}: {e2}", record_path.display()))
                    }
                }
            }
            Err(e) => return Err(format!("failed to read {}: {e}", record_path.display())),
        };
        records.push(LocatedRecord { record, dir: path });
    }
    records.sort_by(|a, b| {
        a.record
            .started_at
            .cmp(&b.record.started_at)
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
    Ok(records)
}

pub fn list_records_for_root(
    root: &Path,
    state_dir: Option<&Path>,
) -> Result<Vec<LocatedRecord>, String> {
    let mut records = list_records_in(&surfaces_dir(root))?;
    if let Some(sd) = state_dir {
        if let Some(store) = existing_folder_store(sd, root)? {
            records.extend(list_records_in(&store.join("surfaces"))?);
        }
    }
    records.sort_by(|a, b| {
        a.record
            .started_at
            .cmp(&b.record.started_at)
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
    Ok(records)
}

fn pid_exists(pid: u32) -> bool {
    let Ok(raw_pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if raw_pid <= 0 {
        return false;
    }
    let ret = unsafe { libc::kill(raw_pid, 0) };
    if ret == 0 {
        return true;
    }
    !matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    )
}

fn probe_once(port: u16, id: &str, timeout: Duration) -> Liveness {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => return Liveness::NotRunning,
        Err(_) => return Liveness::Unknown,
    };
    if stream.set_read_timeout(Some(timeout)).is_err() {
        return Liveness::Unknown;
    }
    if stream.set_write_timeout(Some(timeout)).is_err() {
        return Liveness::Unknown;
    }
    let req =
        format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return Liveness::Unknown;
    }
    let mut raw = Vec::new();
    if stream.read_to_end(&mut raw).is_err() {
        return Liveness::Unknown;
    }
    let Ok(text) = std::str::from_utf8(&raw) else {
        return Liveness::NotRunning;
    };
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return Liveness::NotRunning;
    };
    let status_line = head.lines().next().unwrap_or("");
    if !(status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200")) {
        return Liveness::NotRunning;
    }
    let Ok(val) = serde_json::from_str::<serde_json::Value>(body) else {
        return Liveness::NotRunning;
    };
    if val.get("surface_id").and_then(|v| v.as_str()) == Some(id) {
        Liveness::Running
    } else {
        Liveness::NotRunning
    }
}

pub fn probe(pid: u32, port: u16, id: &str) -> Liveness {
    if port == 0 || !pid_exists(pid) {
        return Liveness::NotRunning;
    }
    match probe_once(port, id, PROBE_TIMEOUT) {
        Liveness::Unknown => probe_once(port, id, PROBE_RETRY_TIMEOUT),
        other => other,
    }
}

/// Returns true when the surface is running or its liveness cannot be determined.
///
/// An `Unknown` probe result is treated as running so a busy surface keeps its
/// name and `blyte clean` leaves it alone.
pub fn is_running(record: &SurfaceRecord) -> bool {
    matches!(
        probe(record.pid, record.port, &record.id),
        Liveness::Running | Liveness::Unknown
    )
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
