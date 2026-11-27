//! Exclusive one-writer lock for a folder, backed by `flock`.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub kind: String,
    pub id: String,
}

#[derive(Debug)]
pub struct FolderLock {
    #[cfg(unix)]
    _file: std::fs::File,
}

#[cfg(unix)]
fn read_holder(file: &mut std::fs::File) -> Result<Holder, String> {
    use std::io::{Read, Seek, SeekFrom};

    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("cannot seek lock file: {e}"))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| format!("cannot read lock file: {e}"))?;
    let first_line = text.lines().next().unwrap_or("");
    serde_json::from_str(first_line).map_err(|e| format!("cannot parse lock holder: {e}"))
}

#[cfg(unix)]
pub fn acquire(lock_path: &Path, holder: &Holder) -> Result<FolderLock, String> {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::io::AsRawFd;

    if let Some(parent) = lock_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create lock directory {}: {e}", parent.display()))?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| format!("cannot open lock file {}: {e}", lock_path.display()))?;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        let line = serde_json::to_string(holder)
            .map_err(|e| format!("cannot serialize lock holder: {e}"))?;
        file.set_len(0)
            .map_err(|e| format!("cannot truncate lock file {}: {e}", lock_path.display()))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("cannot seek lock file {}: {e}", lock_path.display()))?;
        writeln!(file, "{line}")
            .map_err(|e| format!("cannot write lock file {}: {e}", lock_path.display()))?;
        file.flush()
            .map_err(|e| format!("cannot flush lock file {}: {e}", lock_path.display()))?;
        return Ok(FolderLock { _file: file });
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN) {
        return match read_holder(&mut file) {
            Ok(h) => Err(format!(
                "{} {} (pid {}) is already working in this folder",
                h.kind, h.id, h.pid
            )),
            Err(_) => Err(format!(
                "this folder is already locked ({})",
                lock_path.display()
            )),
        };
    }
    Err(format!("cannot lock {}: {err}", lock_path.display()))
}

#[cfg(unix)]
pub fn holder(lock_path: &Path) -> Result<Option<Holder>, String> {
    use std::io::ErrorKind;
    use std::os::unix::io::AsRawFd;

    let mut file = match std::fs::OpenOptions::new().read(true).open(lock_path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "cannot open lock file {}: {e}",
                lock_path.display()
            ))
        }
    };
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        return Ok(None);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN) {
        return read_holder(&mut file).map(Some).map_err(|e| {
            format!(
                "folder is locked ({}), but holder could not be read: {e}",
                lock_path.display()
            )
        });
    }
    Err(format!(
        "cannot inspect lock file {}: {err}",
        lock_path.display()
    ))
}

#[cfg(not(unix))]
pub fn acquire(lock_path: &Path, _holder: &Holder) -> Result<FolderLock, String> {
    Err(format!(
        "folder locking is not supported on this platform ({})",
        lock_path.display()
    ))
}

#[cfg(not(unix))]
pub fn holder(lock_path: &Path) -> Result<Option<Holder>, String> {
    Err(format!(
        "folder locking is not supported on this platform ({})",
        lock_path.display()
    ))
}
