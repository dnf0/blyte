//! Machine-wide gate slots so parallel dispatches do not all build and test at once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Environment variable set on gate commands so nested runs skip slot acquisition.
pub const IN_GATE_ENV: &str = "__BLYTE_IN_GATE";

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable machine-wide gate slots for this process.
pub fn enable() {
    ENABLED.store(true, Ordering::SeqCst);
}

/// Returns whether machine-wide gate slots are enabled for this process.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Returns whether this process is running inside an enclosing gate command.
pub fn nested() -> bool {
    std::env::var_os(IN_GATE_ENV).is_some()
}

/// Returns the maximum number of simultaneous gate slots allowed machine-wide.
///
/// Configured via `BLYTE_GATE_SLOTS` when set to an integer ≥ 1. Defaults to an eighth of the
/// available parallelism, at least 1. Measured 2026-09-27 on 12 cores, three cold `cargo test`
/// builds: all at once finished at 33/33/49 s, one at a time at 14/30/47 s. The same total, but
/// the first result 2.4x sooner and the mean 21% sooner, because cargo already uses every core.
/// A quarter of the cores (3 here) would never have queued them.
pub fn slots() -> usize {
    if let Ok(val) = std::env::var("BLYTE_GATE_SLOTS") {
        if let Ok(n) = val.parse::<usize>() {
            if n >= 1 {
                return n;
            }
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get() / 8)
        .unwrap_or(1)
        .max(1)
}

/// Returns the directory used for machine-wide gate slot lock files.
///
/// Configured via `BLYTE_GATE_SLOT_DIR` when non-empty. Defaults to `temp_dir()/blyte-gate-slots`.
pub fn slot_dir() -> PathBuf {
    match std::env::var("BLYTE_GATE_SLOT_DIR") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => std::env::temp_dir().join("blyte-gate-slots"),
    }
}

/// A guard holding an open gate slot lock file. Dropping releases the slot.
#[derive(Debug)]
pub struct SlotGuard {
    index: usize,
    #[cfg(unix)]
    _file: std::fs::File,
}

impl SlotGuard {
    pub fn index(&self) -> usize {
        self.index
    }
}

/// Acquire one of `max` gate slots in `dir`, sleeping `poll` between attempts.
///
/// Returns the acquired slot guard and the total duration spent waiting.
#[cfg(unix)]
pub fn acquire(max: usize, dir: &Path, poll: Duration) -> std::io::Result<(SlotGuard, Duration)> {
    use std::os::unix::io::AsRawFd;

    std::fs::create_dir_all(dir)?;
    let max = max.max(1);
    let start = Instant::now();
    let mut had_to_wait = false;

    loop {
        for index in 0..max {
            let path = dir.join(format!("slot-{index}.lock"));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if ret == 0 {
                let waited = if had_to_wait {
                    start.elapsed()
                } else {
                    Duration::ZERO
                };
                return Ok((SlotGuard { index, _file: file }, waited));
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK)
                || err.raw_os_error() == Some(libc::EAGAIN)
            {
                continue;
            }
            return Err(err);
        }
        had_to_wait = true;
        std::thread::sleep(poll);
    }
}

/// On non-unix targets, returns a guard at once with zero wait.
#[cfg(not(unix))]
pub fn acquire(
    _max: usize,
    _dir: &Path,
    _poll: Duration,
) -> std::io::Result<(SlotGuard, Duration)> {
    Ok((SlotGuard { index: 0 }, Duration::ZERO))
}
