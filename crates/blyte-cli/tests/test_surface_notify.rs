use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blyte_cli::serve::TOKEN_HEADER;
use blyte_cli::surface_notify::{notify_runs_changed, report, NotifyOutcome};

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn write_surface_files(root: &Path, port: u16, token: &str) {
    write_file(
        &root.join(".wt/surface.json"),
        &format!(
            r#"{{"pid":12345,"port":{port},"session_id":"surface-test","started_at":1700000000}}"#
        ),
    );
    write_file(&root.join(".wt/surface.token"), &format!("{token}\n"));
}

fn spawn_single_response_listener(
    status_line: &'static str,
    body: &'static str,
) -> (u16, Arc<Mutex<Option<String>>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let recorded = Arc::new(Mutex::new(None));
    let recorded_clone = Arc::clone(&recorded);
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        let req_text = String::from_utf8_lossy(&buf).into_owned();
        *recorded_clone.lock().unwrap() = Some(req_text);
        let resp = format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    });
    (port, recorded, handle)
}

#[test]
fn no_surface_files_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = notify_runs_changed(dir.path(), "T1");
    assert_eq!(outcome, NotifyOutcome::NoSurface);

    let mut err = Vec::new();
    report(&outcome, &mut err);
    assert!(err.is_empty(), "expected no stderr output, got: {err:?}");
}

#[test]
fn a_running_surface_is_told() {
    let dir = tempfile::tempdir().unwrap();
    let token = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    let (port, recorded, handle) = spawn_single_response_listener("HTTP/1.1 202 Accepted", "{}");
    write_surface_files(dir.path(), port, token);

    let outcome = notify_runs_changed(dir.path(), "T2");
    handle.join().unwrap();

    assert_eq!(outcome, NotifyOutcome::Told);
    let mut err = Vec::new();
    report(&outcome, &mut err);
    assert!(err.is_empty());

    let raw_req = recorded
        .lock()
        .unwrap()
        .clone()
        .expect("listener did not receive a request");
    let first_line = raw_req.lines().next().unwrap_or("");
    assert!(
        first_line.starts_with("POST /api/runs/changed HTTP/1.1"),
        "unexpected request line: {first_line}"
    );
    let expected_header = format!("{TOKEN_HEADER}: {token}");
    assert!(
        raw_req
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&expected_header)),
        "expected token header in request, got: {raw_req}"
    );
}

#[test]
fn a_dead_surface_is_reported_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = listener.local_addr().unwrap().port();
    drop(listener);

    let secret_token = "deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567";
    write_surface_files(dir.path(), dead_port, secret_token);

    let outcome = notify_runs_changed(dir.path(), "RUN-DEAD-1");
    let msg = match &outcome {
        NotifyOutcome::Failed(m) => m.clone(),
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(
        msg.contains(&format!("127.0.0.1:{dead_port}")),
        "expected port {dead_port} in message: {msg}"
    );
    assert!(
        msg.contains("RUN-DEAD-1"),
        "expected run id in message: {msg}"
    );
    assert!(
        msg.contains("within 30 s"),
        "expected safety-net mention in message: {msg}"
    );
    assert!(
        !msg.contains(secret_token),
        "token must never appear in error message: {msg}"
    );

    let mut err = Vec::new();
    report(&outcome, &mut err);
    let err_str = String::from_utf8(err).unwrap();
    assert_eq!(err_str, format!("blyte: {msg}\n"));
    assert!(!err_str.contains(secret_token));
}

#[test]
fn an_unreadable_surface_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let surface_path = dir.path().join(".wt/surface.json");
    write_file(&surface_path, "not json");

    let outcome = notify_runs_changed(dir.path(), "T3");
    let msg = match &outcome {
        NotifyOutcome::Failed(m) => m.clone(),
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(
        msg.contains("surface.json"),
        "expected surface.json in message: {msg}"
    );

    let mut err = Vec::new();
    report(&outcome, &mut err);
    let err_str = String::from_utf8(err).unwrap();
    assert_eq!(err_str, format!("blyte: {msg}\n"));
}

#[test]
fn a_wrong_token_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let secret_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let (port, _recorded, handle) =
        spawn_single_response_listener("HTTP/1.1 403 Forbidden", r#"{"error":"forbidden"}"#);
    write_surface_files(dir.path(), port, secret_token);

    let outcome = notify_runs_changed(dir.path(), "T4");
    handle.join().unwrap();

    let msg = match &outcome {
        NotifyOutcome::Failed(m) => m.clone(),
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(
        msg.contains(&format!("127.0.0.1:{port}")),
        "expected port {port} in message: {msg}"
    );
    assert!(msg.contains("T4"), "expected run id in message: {msg}");
    assert!(msg.contains("403"), "expected status 403 in message: {msg}");
    assert!(
        msg.contains("within 30 s"),
        "expected safety-net mention in message: {msg}"
    );
    assert!(
        !msg.contains(secret_token),
        "token must never appear in error message: {msg}"
    );
}
