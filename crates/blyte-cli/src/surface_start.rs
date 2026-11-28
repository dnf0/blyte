use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

const START_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const TAIL_LINES: usize = 20;

pub const STRIPPED_ENV_VARS: &[&str] = &[
    "BLYTE_SESSION",
    "BLYTE_DISPATCH_ID",
    "BLYTE_DISPATCH_PARENT",
    "BLYTE_MODEL_SHELL",
    "__BLYTE_IN_GATE",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedSurface {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    BadRequest(String),
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedStartLine {
    name: String,
    port: u16,
    token: String,
}

pub fn resolve_start_folder(raw: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME");
    resolve_start_folder_with_home(raw, home.as_deref())
}

pub fn resolve_start_folder_with_home(raw: &str, home: Option<&OsStr>) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("folder must not be empty".to_string());
    }

    let path = if trimmed == "~" {
        let home_dir = home.filter(|v| !v.is_empty()).ok_or_else(|| {
            format!("cannot expand folder {trimmed}: HOME is not set or is empty")
        })?;
        PathBuf::from(home_dir)
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let home_dir = home.filter(|v| !v.is_empty()).ok_or_else(|| {
            format!("cannot expand folder {trimmed}: HOME is not set or is empty")
        })?;
        PathBuf::from(home_dir).join(rest.trim_start_matches('/'))
    } else {
        PathBuf::from(trimmed)
    };

    if !path.is_absolute() {
        return Err(format!(
            "folder {} must be an absolute path",
            path.display()
        ));
    }

    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(format!("folder {} does not exist", path.display()));
        }
        Err(e) => {
            return Err(format!("cannot inspect folder {}: {e}", path.display()));
        }
    };

    if !meta.is_dir() {
        return Err(format!("folder {} is not a directory", path.display()));
    }

    Ok(path)
}

fn random_hex_16() -> Result<String, String> {
    let mut bytes = [0u8; 8];
    let mut file =
        fs::File::open("/dev/urandom").map_err(|e| format!("failed to open /dev/urandom: {e}"))?;
    file.read_exact(&mut bytes)
        .map_err(|e| format!("failed to read /dev/urandom: {e}"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(16);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

fn create_log_file(state_dir: &Path) -> Result<(PathBuf, fs::File), String> {
    let dir = state_dir.join("surface-starts");
    #[cfg(unix)]
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("failed to set permissions on {}: {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }

    for _ in 0..16 {
        let name = format!("{}.log", random_hex_16()?);
        let path = dir.join(name);
        let mut opts = OpenOptions::new();
        opts.create_new(true).read(true).write(true);
        #[cfg(unix)]
        opts.mode(0o600);
        match opts.open(&path) {
            Ok(file) => {
                #[cfg(unix)]
                {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| {
                        let _ = fs::remove_file(&path);
                        format!("failed to set permissions on {}: {e}", path.display())
                    })?;
                }
                return Ok((path, file));
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!("failed to create log file {}: {e}", path.display()));
            }
        }
    }

    Err(format!(
        "failed to create a unique log file in {}",
        dir.display()
    ))
}

fn parse_start_line(line: &str) -> Option<ParsedStartLine> {
    let trimmed = line.trim_end_matches('\r');
    let rest = trimmed.strip_prefix("blyte: surface ")?;
    let (name, url) = rest.rsplit_once(" at ")?;
    if name.is_empty() {
        return None;
    }
    let after_host = url.strip_prefix("http://127.0.0.1:")?;
    let (port_str, token) = after_host.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    if port == 0 {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(ParsedStartLine {
        name: name.to_string(),
        port,
        token: token.to_string(),
    })
}

fn find_start_line_in_complete_lines(text: &str) -> Option<ParsedStartLine> {
    let mut rest = text;
    while let Some((line, after)) = rest.split_once('\n') {
        if let Some(parsed) = parse_start_line(line) {
            return Some(parsed);
        }
        rest = after;
    }
    None
}

fn redact_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("#token=") {
        out.push_str(&rest[..idx]);
        out.push_str("#token=<redacted>");
        let after = &rest[idx + "#token=".len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']'))
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn last_lines_redacted(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let joined = lines[start..].join("\n");
    redact_tokens(&joined)
}

pub fn start_surface_in(raw_folder: &str, state_dir: &Path) -> Result<StartedSurface, StartError> {
    let folder = resolve_start_folder(raw_folder).map_err(StartError::BadRequest)?;
    let exe = std::env::current_exe()
        .map_err(|e| StartError::Internal(format!("failed to locate blyte executable: {e}")))?;
    start_surface_with_exe(&folder, state_dir, &exe, &["--no-open"], START_TIMEOUT)
}

pub fn start_surface_with_exe(
    folder: &Path,
    state_dir: &Path,
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<StartedSurface, StartError> {
    let (log_path, stdout_file) = create_log_file(state_dir).map_err(StartError::Internal)?;
    let stderr_file = match stdout_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            let _ = fs::remove_file(&log_path);
            return Err(StartError::Internal(format!(
                "failed to duplicate log file handle: {e}"
            )));
        }
    };

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    for var in STRIPPED_ENV_VARS {
        cmd.env_remove(var);
    }

    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = fs::remove_file(&log_path);
            return Err(StartError::Internal(format!(
                "failed to start blyte in {}: {e}",
                folder.display()
            )));
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(bytes) = fs::read(&log_path) {
            let text = String::from_utf8_lossy(&bytes);
            if let Some(parsed) = find_start_line_in_complete_lines(&text) {
                let _ = fs::remove_file(&log_path);
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                let code = crate::surfaces_cmd::request_open_code_at(
                    &parsed.name,
                    parsed.port,
                    &parsed.token,
                )
                .map_err(StartError::Internal)?;
                return Ok(StartedSurface {
                    name: parsed.name,
                    url: format!("http://127.0.0.1:{}/#open={code}", parsed.port),
                });
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let bytes = match fs::read(&log_path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        let _ = fs::remove_file(&log_path);
                        return Err(StartError::Internal(format!(
                            "surface in {} exited before starting ({status}); failed to read start log {}: {e}",
                            folder.display(),
                            log_path.display()
                        )));
                    }
                };
                let text = String::from_utf8_lossy(&bytes);
                let _ = fs::remove_file(&log_path);
                let tail = last_lines_redacted(&text, TAIL_LINES);
                let msg = if tail.trim().is_empty() {
                    format!(
                        "surface in {} exited before starting ({status})",
                        folder.display()
                    )
                } else {
                    format!(
                        "surface in {} exited before starting ({status}):\n{tail}",
                        folder.display()
                    )
                };
                return Err(StartError::Internal(msg));
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&log_path);
                return Err(StartError::Internal(format!(
                    "failed to check started surface process: {e}"
                )));
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StartError::Internal(format!(
                "timed out after {} s waiting for surface in {} to start; log kept at {}",
                timeout.as_secs(),
                folder.display(),
                log_path.display()
            )));
        }
        std::thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_folder_checks_tilde_absolute_exists_and_is_dir() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let sub = home.join("proj");
        fs::create_dir_all(&sub).unwrap();
        let file = temp.path().join("file.txt");
        fs::write(&file, "hi").unwrap();
        let missing = temp.path().join("nope");

        let resolved = resolve_start_folder_with_home("~/proj", Some(home.as_os_str())).unwrap();
        assert_eq!(resolved, sub);

        let err_no_home = resolve_start_folder_with_home("~/proj", None).unwrap_err();
        assert!(err_no_home.contains("~/proj"), "{err_no_home}");
        assert!(err_no_home.contains("HOME"), "{err_no_home}");

        let err_empty_home =
            resolve_start_folder_with_home("~/proj", Some(OsStr::new(""))).unwrap_err();
        assert!(err_empty_home.contains("HOME"), "{err_empty_home}");

        let err_rel = resolve_start_folder_with_home("relative/dir", None).unwrap_err();
        assert!(err_rel.contains("relative/dir"), "{err_rel}");
        assert!(err_rel.contains("absolute"), "{err_rel}");

        let err_missing =
            resolve_start_folder_with_home(&missing.to_string_lossy(), None).unwrap_err();
        assert!(
            err_missing.contains(&missing.display().to_string()),
            "{err_missing}"
        );
        assert!(err_missing.contains("does not exist"), "{err_missing}");
        assert!(!missing.exists());

        let err_file = resolve_start_folder_with_home(&file.to_string_lossy(), None).unwrap_err();
        assert!(err_file.contains(&file.display().to_string()), "{err_file}");
        assert!(err_file.contains("not a directory"), "{err_file}");
    }

    #[test]
    fn parse_and_redact_start_line() {
        let line = "blyte: surface demo at http://127.0.0.1:4321/#token=secret123\n";
        let parsed = find_start_line_in_complete_lines(line).unwrap();
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.port, 4321);
        assert_eq!(parsed.token, "secret123");

        assert!(find_start_line_in_complete_lines(
            "blyte: surface demo at http://127.0.0.1:4321/#token=partial"
        )
        .is_none());

        let redacted = redact_tokens("http://127.0.0.1:4321/#token=secret123 done");
        assert!(!redacted.contains("secret123"), "{redacted}");
        assert!(redacted.contains("#token=<redacted>"), "{redacted}");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_child_and_keeps_log_file() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let folder = temp.path().join("work");
        fs::create_dir_all(&folder).unwrap();

        let err = start_surface_with_exe(
            &folder,
            &state,
            Path::new("/bin/sleep"),
            &["30"],
            Duration::from_millis(150),
        )
        .unwrap_err();
        let StartError::Internal(msg) = err else {
            panic!("expected internal error, got {err:?}");
        };
        assert!(msg.contains("timed out"), "{msg}");
        let log_dir = state.join("surface-starts");
        let entries: Vec<_> = fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let log_path = entries[0].path();
        assert!(msg.contains(&log_path.display().to_string()), "{msg}");
        let dir_mode = fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&log_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn early_exit_reports_unreadable_start_log() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let folder = temp.path().join("work");
        fs::create_dir_all(&folder).unwrap();
        let log_dir = state.join("surface-starts");
        let log_dir_str = log_dir.to_string_lossy().into_owned();

        let err = start_surface_with_exe(
            &folder,
            &state,
            Path::new("/bin/sh"),
            &["-c", "rm -f \"$1\"/*.log; exit 1", "sh", &log_dir_str],
            Duration::from_secs(5),
        )
        .unwrap_err();
        let StartError::Internal(msg) = err else {
            panic!("expected internal error, got {err:?}");
        };
        assert!(msg.contains("failed to read start log"), "{msg}");
        assert!(msg.contains(&log_dir_str), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn early_exit_removes_start_log_file() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let folder = temp.path().join("work");
        fs::create_dir_all(&folder).unwrap();

        let err = start_surface_with_exe(
            &folder,
            &state,
            Path::new("/bin/sh"),
            &["-c", "echo boom >&2; exit 1"],
            Duration::from_secs(5),
        )
        .unwrap_err();
        let StartError::Internal(msg) = err else {
            panic!("expected internal error, got {err:?}");
        };
        assert!(msg.contains("boom"), "{msg}");
        let log_dir = state.join("surface-starts");
        assert!(log_dir.is_dir());
        let remaining: Vec<_> = fs::read_dir(&log_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert!(remaining.is_empty(), "leftover log files: {remaining:?}");
    }
}
