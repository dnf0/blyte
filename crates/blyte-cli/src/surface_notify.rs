use serde::Deserialize;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::serve::TOKEN_HEADER;

const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESPONSE_HEAD_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyOutcome {
    NoSurface,
    Told,
    Failed(String),
}

#[derive(Deserialize)]
struct StartSurface {
    surface: Option<String>,
}

fn format_notify_failure(port: u16, id: &str, reason: &str) -> NotifyOutcome {
    NotifyOutcome::Failed(format!(
        "could not tell the surface at 127.0.0.1:{port} that {id} finished: {reason}; it checks again within 30 s"
    ))
}

fn parse_status_code(head_bytes: &[u8]) -> Result<u16, String> {
    let text = std::str::from_utf8(head_bytes)
        .map_err(|_| "response status line is not valid UTF-8".to_string())?;
    let status_line = text.split('\n').next().unwrap_or("").trim_end_matches('\r');
    let mut parts = status_line.split_whitespace();
    let version = parts
        .next()
        .ok_or_else(|| "empty HTTP response".to_string())?;
    if !version.starts_with("HTTP/") {
        return Err("invalid HTTP response".to_string());
    }
    let code_str = parts
        .next()
        .ok_or_else(|| "missing HTTP status code".to_string())?;
    code_str
        .parse::<u16>()
        .map_err(|_| "invalid HTTP status code".to_string())
}

fn post_runs_changed(port: u16, token: &str) -> Result<u16, String> {
    let deadline = Instant::now() + NOTIFY_TIMEOUT;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, NOTIFY_TIMEOUT)
        .map_err(|e| format!("connection failed: {e}"))?;

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("timed out".to_string());
    }
    let _ = stream.set_write_timeout(Some(remaining));

    let req = format!(
        "POST /api/runs/changed HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if raw.contains(&b'\n') {
            break;
        }
        if raw.len() >= MAX_RESPONSE_HEAD_BYTES {
            return Err("response status line too long".to_string());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("timed out".to_string());
        }
        let _ = stream.set_read_timeout(Some(remaining));
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::WouldBlock => {
                return Err("timed out".to_string());
            }
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }

    parse_status_code(&raw)
}

fn notify_runs_changed_in(records_dir: &Path, surfaces_root: &Path, id: &str) -> NotifyOutcome {
    let start_path = records_dir.join(format!("{id}.start.json"));
    let start_raw = match std::fs::read_to_string(&start_path) {
        Ok(s) => s,
        Err(e) => {
            return NotifyOutcome::Failed(format!("cannot read {}: {e}", start_path.display()));
        }
    };
    let start: StartSurface = match serde_json::from_str(&start_raw) {
        Ok(v) => v,
        Err(e) => {
            return NotifyOutcome::Failed(format!("cannot parse {}: {e}", start_path.display()));
        }
    };
    let Some(sid) = start.surface else {
        return NotifyOutcome::NoSurface;
    };
    if let Err(e) = blyte_agent::sandbox::validate_surface_id(&sid) {
        return NotifyOutcome::Failed(e);
    }
    let surface_dir = surfaces_root.join(&sid);
    if !surface_dir.is_dir() {
        return NotifyOutcome::Failed(format!(
            "run {id} belongs to surface {sid}, which has no directory under {}",
            surfaces_root.display()
        ));
    }
    let record = match crate::surface_dir::read_record_in(&surface_dir, &sid) {
        Ok(r) => r,
        Err(e) => return NotifyOutcome::Failed(e),
    };

    let token_path = match blyte_agent::sandbox::surface_token_path_for_id(&sid) {
        Ok(p) => p,
        Err(e) => return NotifyOutcome::Failed(e),
    };

    let token_raw = match std::fs::read_to_string(&token_path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return NotifyOutcome::NoSurface,
        Err(e) => {
            return NotifyOutcome::Failed(format!("cannot read {}: {e}", token_path.display()));
        }
    };
    let token = token_raw.trim();
    if token.is_empty() {
        return NotifyOutcome::Failed(format!("{} is empty", token_path.display()));
    }
    if token.contains(['\r', '\n']) {
        return NotifyOutcome::Failed(format!("invalid token in {}", token_path.display()));
    }

    match post_runs_changed(record.port, token) {
        Ok(202) => NotifyOutcome::Told,
        Ok(status) => {
            format_notify_failure(record.port, id, &format!("unexpected HTTP status {status}"))
        }
        Err(reason) => format_notify_failure(record.port, id, &reason),
    }
}

pub fn notify_runs_changed(main_checkout: &Path, id: &str) -> NotifyOutcome {
    match crate::serve::is_repository(main_checkout) {
        Ok(true) => {
            return notify_runs_changed_in(
                &main_checkout.join(".wt"),
                &crate::surface_dir::surfaces_dir(main_checkout),
                id,
            );
        }
        Ok(false) => {}
        Err(e) => return NotifyOutcome::Failed(e),
    }
    let state_dir = match crate::surface_registry::state_dir() {
        Ok(d) => d,
        Err(e) => return NotifyOutcome::Failed(e),
    };
    match crate::surface_dir::existing_folder_store(&state_dir, main_checkout) {
        Ok(Some(store)) => {
            return notify_runs_changed_in(&store.join("runs"), &store.join("surfaces"), id);
        }
        Ok(None) => {}
        Err(e) => return NotifyOutcome::Failed(e),
    }
    if main_checkout.join(".wt").is_dir() {
        return notify_runs_changed_in(
            &main_checkout.join(".wt"),
            &crate::surface_dir::surfaces_dir(main_checkout),
            id,
        );
    }
    let start_path = main_checkout.join(".wt").join(format!("{id}.start.json"));
    NotifyOutcome::Failed(format!(
        "cannot read {}: no run store",
        start_path.display()
    ))
}

pub fn report(outcome: &NotifyOutcome, err: &mut dyn Write) {
    if let NotifyOutcome::Failed(msg) = outcome {
        let _ = writeln!(err, "blyte: {msg}");
    }
}
