use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blyte_agent::sandbox::surface_token_path_for_id;
use blyte_cli::serve::TOKEN_HEADER;
use blyte_cli::surface_dir::{surface_dir, write_record_in, SurfaceRecord};
use blyte_cli::surface_notify::{notify_runs_changed, report, NotifyOutcome};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct StateHomeGuard {
    prev: Option<OsString>,
    _dir: tempfile::TempDir,
}

impl StateHomeGuard {
    fn new() -> Self {
        let dir = repo_dir();
        let prev = std::env::var_os("XDG_STATE_HOME");
        std::env::set_var("XDG_STATE_HOME", dir.path());
        Self { prev, _dir: dir }
    }
}

impl Drop for StateHomeGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// A temporary git repository: its run records live in `.wt`.
fn repo_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "git init failed");
    dir
}

fn write_start_record(root: &Path, id: &str, surface: Option<&str>) {
    let val = match surface {
        Some(sid) => serde_json::json!({ "id": id, "surface": sid }),
        None => serde_json::json!({ "id": id, "surface": serde_json::Value::Null }),
    };
    write_file(
        &root.join(".wt").join(format!("{id}.start.json")),
        &val.to_string(),
    );
}

fn write_surface_files(root: &Path, sid: &str, name: &str, port: u16, token: Option<&str>) {
    write_record_in(
        &surface_dir(root, sid),
        &SurfaceRecord {
            id: sid.to_string(),
            name: name.to_string(),
            pid: 12345,
            port,
            session_id: format!("session-{sid}"),
            started_at: 1_700_000_000,
        },
    )
    .unwrap();
    if let Some(tok) = token {
        let token_path = surface_token_path_for_id(sid).unwrap();
        write_file(&token_path, &format!("{tok}\n"));
    }
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
fn start_record_with_null_surface_is_no_surface() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    write_start_record(dir.path(), "T1", None);

    let outcome = notify_runs_changed(dir.path(), "T1");
    assert_eq!(outcome, NotifyOutcome::NoSurface);

    let mut err = Vec::new();
    report(&outcome, &mut err);
    assert!(err.is_empty(), "expected no stderr output, got: {err:?}");
}

#[test]
fn missing_start_record_is_failed() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();

    let outcome = notify_runs_changed(dir.path(), "T_MISSING");
    match outcome {
        NotifyOutcome::Failed(msg) => {
            assert!(
                msg.contains("cannot read"),
                "expected cannot read in message: {msg}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn a_running_surface_is_told() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    let sid = "s0123456789abcdef";
    let token = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    let (port, recorded, handle) = spawn_single_response_listener("HTTP/1.1 202 Accepted", "{}");
    write_start_record(dir.path(), "T2", Some(sid));
    write_surface_files(dir.path(), sid, "demo", port, Some(token));

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
fn two_surfaces_only_the_owner_is_told() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();

    let sid1 = "s1111111111111111";
    let sid2 = "s2222222222222222";
    let tok1 = "1111111111111111111111111111111111111111111111111111111111111111";
    let tok2 = "2222222222222222222222222222222222222222222222222222222222222222";

    let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
    listener1.set_nonblocking(true).unwrap();
    let port1 = listener1.local_addr().unwrap().port();
    let (port2, recorded2, handle2) = spawn_single_response_listener("HTTP/1.1 202 Accepted", "{}");

    write_surface_files(dir.path(), sid1, "demo", port1, Some(tok1));
    write_surface_files(dir.path(), sid2, "demo-2", port2, Some(tok2));
    write_start_record(dir.path(), "T_TWO", Some(sid2));

    let outcome = notify_runs_changed(dir.path(), "T_TWO");
    handle2.join().unwrap();

    assert_eq!(outcome, NotifyOutcome::Told);
    assert!(
        listener1.accept().is_err(),
        "first surface listener must not receive a connection"
    );
    let raw_req = recorded2
        .lock()
        .unwrap()
        .clone()
        .expect("second surface listener did not receive a request");
    let expected_header = format!("{TOKEN_HEADER}: {tok2}");
    assert!(
        raw_req
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&expected_header)),
        "expected second surface token in request, got: {raw_req}"
    );
}

#[test]
fn start_record_naming_missing_surface_directory_is_failed() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    let sid = "sdeadbeefcafebabe";
    write_start_record(dir.path(), "T_NODIR", Some(sid));

    let outcome = notify_runs_changed(dir.path(), "T_NODIR");
    let msg = match outcome {
        NotifyOutcome::Failed(m) => m,
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(
        msg.contains("has no directory"),
        "expected 'has no directory' in message: {msg}"
    );
    assert!(msg.contains(sid), "expected surface id in message: {msg}");
}

#[test]
fn record_present_without_token_file_is_no_surface() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    let sid = "sabcdef0123456789";
    write_start_record(dir.path(), "T_STOPPED", Some(sid));
    write_surface_files(dir.path(), sid, "demo", 12345, None);

    let outcome = notify_runs_changed(dir.path(), "T_STOPPED");
    assert_eq!(outcome, NotifyOutcome::NoSurface);
}

#[test]
fn a_dead_surface_is_reported_not_fatal() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = listener.local_addr().unwrap().port();
    drop(listener);

    let sid = "sfedcba9876543210";
    let secret_token = "deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567";
    write_start_record(dir.path(), "RUN-DEAD-1", Some(sid));
    write_surface_files(dir.path(), sid, "demo", dead_port, Some(secret_token));

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
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    let sid = "saaaa1111bbbb2222";
    write_start_record(dir.path(), "T3", Some(sid));
    let surface_path = dir
        .path()
        .join(".wt/surfaces")
        .join(sid)
        .join("surface.json");
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
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _state = StateHomeGuard::new();
    let dir = repo_dir();
    let sid = "scccc3333dddd4444";
    let secret_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let (port, _recorded, handle) =
        spawn_single_response_listener("HTTP/1.1 403 Forbidden", r#"{"error":"forbidden"}"#);
    write_start_record(dir.path(), "T4", Some(sid));
    write_surface_files(dir.path(), sid, "demo", port, Some(secret_token));

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
