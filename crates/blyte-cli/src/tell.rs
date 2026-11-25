use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::serve::MESSAGE_HEADER;

const TELL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TellOutcome {
    Delivered,
    Held,
}

fn format_outcome(name: &str, outcome: TellOutcome) -> String {
    match outcome {
        TellOutcome::Delivered => format!("delivered to {name}"),
        TellOutcome::Held => format!(
            "held by {name}: it has run five turns from other surfaces since its user last wrote; the message waits for its user's next message"
        ),
    }
}

fn parse_status_code(head: &str) -> Result<u16, String> {
    let status_line = head.lines().next().unwrap_or("").trim_end_matches('\r');
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

fn post_peer_message(
    target_name: &str,
    port: u16,
    credential: &str,
    body: &str,
) -> Result<(u16, String), String> {
    let deadline = Instant::now() + TELL_TIMEOUT;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, TELL_TIMEOUT).map_err(|e| {
        if e.kind() == ErrorKind::ConnectionRefused {
            format!("surface {target_name} refused the connection on port {port}")
        } else {
            format!("cannot connect to surface {target_name} on port {port}: {e}")
        }
    })?;

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(format!("surface {target_name} timed out on port {port}"));
    }
    let _ = stream.set_write_timeout(Some(remaining));

    let req = format!(
        "POST /api/peer/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{MESSAGE_HEADER}: {credential}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("failed to send message to surface {target_name}: {e}"))?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if raw.len() >= MAX_RESPONSE_BYTES {
            return Err(format!(
                "surface {target_name} gave an answer blyte does not understand"
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("surface {target_name} timed out on port {port}"));
        }
        let _ = stream.set_read_timeout(Some(remaining));
        let max_read = (MAX_RESPONSE_BYTES - raw.len()).min(chunk.len());
        match stream.read(&mut chunk[..max_read]) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::WouldBlock => {
                return Err(format!("surface {target_name} timed out on port {port}"));
            }
            Err(e) => {
                return Err(format!(
                    "failed to read answer from surface {target_name}: {e}"
                ));
            }
        }
    }

    let text = std::str::from_utf8(&raw)
        .map_err(|_| format!("surface {target_name} gave an answer blyte does not understand"))?;
    let (head, resp_body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("surface {target_name} gave an answer blyte does not understand"))?;
    let status = parse_status_code(head)
        .map_err(|_| format!("surface {target_name} gave an answer blyte does not understand"))?;
    Ok((status, resp_body.to_string()))
}

pub fn tell(
    registry_dir: &Path,
    from_name: &str,
    from_repository: &str,
    target: &str,
    text: &str,
) -> Result<TellOutcome, String> {
    let entry = crate::surface_registry::find_by_name(registry_dir, target)?;
    let cred_path = blyte_agent::sandbox::surface_message_path_for_id(&entry.id)?;
    let raw_cred = std::fs::read_to_string(&cred_path).map_err(|_| {
        format!(
            "surface {} has no usable message credential at {}",
            entry.name,
            cred_path.display()
        )
    })?;
    let credential = raw_cred.trim();
    if credential.is_empty() || credential.contains(['\r', '\n']) {
        return Err(format!(
            "surface {} has no usable message credential at {}",
            entry.name,
            cred_path.display()
        ));
    }

    let body = serde_json::to_string(&serde_json::json!({
        "from_name": from_name,
        "from_repository": from_repository,
        "text": text,
    }))
    .map_err(|e| format!("failed to encode peer message: {e}"))?;

    let (status, resp_body) = post_peer_message(&entry.name, entry.port, credential, &body)?;
    if status != 200 {
        let detail = serde_json::from_str::<serde_json::Value>(&resp_body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or_else(|| {
                let trimmed = resp_body.trim();
                if trimmed.is_empty() {
                    "no error message".to_string()
                } else {
                    trimmed.to_string()
                }
            });
        return Err(format!(
            "surface {} answered {status}: {detail}",
            entry.name
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&resp_body).map_err(|_| {
        format!(
            "surface {} gave an answer blyte does not understand",
            entry.name
        )
    })?;
    match parsed.get("status").and_then(|v| v.as_str()) {
        Some("delivered") => Ok(TellOutcome::Delivered),
        Some("held") => Ok(TellOutcome::Held),
        _ => Err(format!(
            "surface {} gave an answer blyte does not understand",
            entry.name
        )),
    }
}

pub fn run_tell(
    registry_dir: &Path,
    target: &str,
    text: &str,
    from: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    if let Err(e) = crate::surface_dir::validate_surface_name(from) {
        let _ = writeln!(err, "blyte: {e}");
        return 1;
    }

    let from_repository = match std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
    {
        Some(name) => name,
        None => {
            let _ = writeln!(err, "blyte: cannot name the current directory");
            return 1;
        }
    };

    match tell(registry_dir, from, &from_repository, target, text) {
        Ok(outcome) => {
            let _ = writeln!(out, "{}", format_outcome(target, outcome));
            0
        }
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            1
        }
    }
}

pub struct SurfacePeerTools {
    pub registry_dir: PathBuf,
    pub surface_id: String,
    pub surface_name: String,
    pub repository: String,
}

impl blyte_agent::tools::PeerTools for SurfacePeerTools {
    fn surfaces(&self) -> Result<String, String> {
        let entries = crate::surface_registry::live_entries(&self.registry_dir)?;
        Ok(crate::surfaces_cmd::format_surfaces_table(
            &entries,
            Some(&self.surface_id),
        ))
    }

    fn tell(&self, name: &str, message: &str) -> Result<String, String> {
        if name == self.surface_name || name == self.surface_id {
            return Err("a surface cannot tell itself".to_string());
        }
        let outcome = tell(
            &self.registry_dir,
            &self.surface_name,
            &self.repository,
            name,
            message,
        )?;
        Ok(format_outcome(name, outcome))
    }
}
