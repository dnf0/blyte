use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const ATTACHMENTS_SUBDIR: &str = "attachments";

static STORE_LOCK: Mutex<()> = Mutex::new(());
static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub image: bool,
}

pub fn detect_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

pub fn mime_type_for(bytes: &[u8]) -> (&'static str, bool) {
    match detect_image(bytes) {
        Some(mime) => (mime, true),
        None => ("application/octet-stream", false),
    }
}

pub fn sanitise_name(raw: &str) -> Result<String, String> {
    let component = raw.rsplit(['/', '\\']).next().unwrap_or("");
    if component.trim().is_empty() {
        return Err("attachment name is empty".to_string());
    }
    let sanitised: String = component
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitised.is_empty() || sanitised == "." || sanitised == ".." {
        return Err("attachment name is empty".to_string());
    }
    Ok(sanitised)
}

pub fn attachments_dir(record_dir: &Path) -> PathBuf {
    record_dir.join(ATTACHMENTS_SUBDIR)
}

pub fn store(dir: &Path, name: &str, bytes: &[u8]) -> Result<Meta, String> {
    store_with_lock(&STORE_LOCK, dir, name, bytes)
}

fn store_with_lock(lock: &Mutex<()>, dir: &Path, name: &str, bytes: &[u8]) -> Result<Meta, String> {
    let sanitised = sanitise_name(name)?;
    if bytes.is_empty() {
        return Err(format!("{sanitised} is empty"));
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "{sanitised} is larger than the 10 MiB limit for one file"
        ));
    }

    let _guard = lock
        .lock()
        .map_err(|_| format!("attachment store lock is poisoned while storing {sanitised}"))?;
    let dest_dir = attachments_dir(dir);
    fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("failed to create {}: {e}", dest_dir.display()))?;

    let next_num = highest_id_in(&dest_dir)? + 1;
    let id = format!("a{next_num}");
    let final_path = dest_dir.join(format!("{id}-{sanitised}"));
    let tmp_name = format!(
        ".tmp-{}-{}-{}",
        id,
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let tmp_path = dest_dir.join(tmp_name);

    let write_res = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("failed to write {}: {e}", tmp_path.display()));
    }
    if let Err(e) = fs::rename(&tmp_path, &final_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "failed to move {} to {}: {e}",
            tmp_path.display(),
            final_path.display()
        ));
    }

    let (mime_type, image) = mime_type_for(bytes);
    Ok(Meta {
        id,
        name: sanitised,
        mime_type: mime_type.to_string(),
        size: bytes.len() as u64,
        image,
    })
}

pub fn load(dir: &Path, id: &str) -> Result<(Meta, Vec<u8>), String> {
    let (meta, _, bytes) = load_with_path(dir, id)?;
    Ok((meta, bytes))
}

pub fn load_with_path(dir: &Path, id: &str) -> Result<(Meta, PathBuf, Vec<u8>), String> {
    if !is_valid_id(id) {
        return Err(format!("unknown attachment id: {id}"));
    }
    let dest_dir = attachments_dir(dir);
    let prefix = format!("{id}-");
    let entries = match fs::read_dir(&dest_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(format!("unknown attachment id: {id}"));
        }
        Err(e) => {
            return Err(format!(
                "failed to read attachments directory {}: {e}",
                dest_dir.display()
            ));
        }
    };

    let mut matched: Option<(String, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to inspect attachment {id}: {e}"))?;
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            continue;
        };
        if let Some(rest) = name_str.strip_prefix(&prefix) {
            if !rest.is_empty() {
                matched = Some((rest.to_string(), entry.path()));
                break;
            }
        }
    }

    let Some((name, path)) = matched else {
        return Err(format!("unknown attachment id: {id}"));
    };
    let abs_path = fs::canonicalize(&path).map_err(|e| {
        format!(
            "failed to canonicalize attachment {id} at {}: {e}",
            path.display()
        )
    })?;
    let bytes = match File::open(&abs_path).and_then(|mut f| {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut buf)?;
        Ok(buf)
    }) {
        Ok(b) => b,
        Err(e) => return Err(format!("attachment {id} is missing or unreadable: {e}")),
    };
    let (mime_type, image) = mime_type_for(&bytes);
    let meta = Meta {
        id: id.to_string(),
        name,
        mime_type: mime_type.to_string(),
        size: bytes.len() as u64,
        image,
    };
    Ok((meta, abs_path, bytes))
}

fn is_valid_id(id: &str) -> bool {
    match id.strip_prefix('a') {
        Some(digits) if !digits.is_empty() => digits.bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

fn highest_id_in(dir: &Path) -> Result<u64, String> {
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("failed to read {}: {e}", dir.display())),
    };
    let mut highest = 0u64;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            continue;
        };
        if name_str.starts_with('.') {
            continue;
        }
        if let Some((prefix, _)) = name_str.split_once('-') {
            if let Some(digits) = prefix.strip_prefix('a') {
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(n) = digits.parse::<u64>() {
                        if n > highest {
                            highest = n;
                        }
                    }
                }
            }
        }
    }
    Ok(highest)
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let (chunks, rem) = bytes.as_chunks::<3>();
    for chunk in chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(n & 0x3f) as usize] as char);
    }
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0xee, 0xdd]), "/+7d");
    }

    #[test]
    fn detect_image_recognises_the_four_formats() {
        assert_eq!(
            detect_image(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0]),
            Some("image/png")
        );
        assert_eq!(detect_image(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(detect_image(b"GIF87a..."), Some("image/gif"));
        assert_eq!(detect_image(b"GIF89a..."), Some("image/gif"));
        assert_eq!(
            detect_image(b"RIFF\x12\x00\x00\x00WEBP"),
            Some("image/webp")
        );
        assert_eq!(detect_image(b"RIFF\x12\x00\x00\x00WAVE"), None);
        assert_eq!(detect_image(b"plain text"), None);
        assert_eq!(detect_image(b""), None);
    }

    #[test]
    fn sanitise_name_strips_paths_and_replaces_other_chars() {
        assert_eq!(sanitise_name("../../x y.png").unwrap(), "x_y.png");
        assert_eq!(sanitise_name(r"C:\tmp\a+b (1).txt").unwrap(), "a_b__1_.txt");
        assert!(sanitise_name("").is_err());
        assert!(sanitise_name("   ").is_err());
        assert!(sanitise_name("..").is_err());
        assert!(sanitise_name("dir/").is_err());
    }

    #[test]
    fn store_continues_after_highest_existing_id() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = store(dir.path(), "first.txt", b"one").unwrap();
        assert_eq!(m1.id, "a1");
        let m2 = store(dir.path(), "second.txt", b"two").unwrap();
        assert_eq!(m2.id, "a2");
        fs::write(attachments_dir(dir.path()).join("a9-manual.txt"), b"nine").unwrap();
        let m3 = store(dir.path(), "third.txt", b"three").unwrap();
        assert_eq!(m3.id, "a10");
        let (loaded, bytes) = load(dir.path(), "a10").unwrap();
        assert_eq!(loaded, m3);
        assert_eq!(bytes, b"three");
    }

    #[test]
    fn store_errors_when_lock_is_poisoned() {
        let dir = tempfile::tempdir().unwrap();
        let lock = std::sync::Arc::new(Mutex::new(()));
        let lock_bg = std::sync::Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let _guard = lock_bg.lock().unwrap();
            panic!("poison test lock");
        })
        .join();
        let err = store_with_lock(&lock, dir.path(), "note.txt", b"hello").unwrap_err();
        assert!(err.contains("poisoned"), "unexpected error: {err}");
        assert!(err.contains("note.txt"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn load_with_path_errors_when_canonicalize_fails() {
        let dir = tempfile::tempdir().unwrap();
        let dest = attachments_dir(dir.path());
        fs::create_dir_all(&dest).unwrap();
        let dangling = dest.join("a1-dangling.txt");
        std::os::unix::fs::symlink(dir.path().join("does-not-exist.txt"), &dangling).unwrap();
        let err = load_with_path(dir.path(), "a1").unwrap_err();
        assert!(err.contains("a1"), "unexpected error: {err}");
        assert!(err.contains("canonicalize"), "unexpected error: {err}");
    }
}
