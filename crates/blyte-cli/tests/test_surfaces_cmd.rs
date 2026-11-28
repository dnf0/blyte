use std::cell::RefCell;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use blyte_agent::sandbox::surface_token_path_for_id;
use blyte_cli::surface_registry::{write_entry, RegistryEntry, SurfaceState};
use blyte_cli::surfaces_cmd::{run_open, run_surfaces};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    prev_xdg: Option<OsString>,
    prev_blyte_state: Option<OsString>,
}

impl EnvGuard {
    fn set_state_root(state_root: &Path) -> Self {
        let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let prev_xdg = std::env::var_os("XDG_STATE_HOME");
        let prev_blyte_state = std::env::var_os("BLYTE_STATE_DIR");
        std::env::set_var("XDG_STATE_HOME", state_root);
        std::env::set_var("BLYTE_STATE_DIR", state_root.join("blyte"));
        Self {
            _lock: lock,
            prev_xdg,
            prev_blyte_state,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev_xdg {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        match &self.prev_blyte_state {
            Some(v) => std::env::set_var("BLYTE_STATE_DIR", v),
            None => std::env::remove_var("BLYTE_STATE_DIR"),
        }
    }
}

struct MockHealthServer {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    recorded_open_code_requests: Arc<Mutex<Vec<String>>>,
}

impl MockHealthServer {
    fn start(surface_id: &str) -> Self {
        Self::start_with_open_code(surface_id, Ok("mock-open-code-abc123".to_string()))
    }

    fn start_with_open_code(surface_id: &str, open_code_reply: Result<String, u16>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_bg = Arc::clone(&recorded);
        let health_body = format!(r#"{{"surface_id":"{surface_id}"}}"#);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let mut req = Vec::new();
                let mut buf = [0u8; 512];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let req_str = String::from_utf8_lossy(&req).to_string();
                let first_line = req_str.lines().next().unwrap_or("");
                let resp = if first_line.starts_with("POST /api/session/open-codes ") {
                    recorded_bg.lock().unwrap().push(req_str);
                    match &open_code_reply {
                        Ok(code) => {
                            let body = format!(r#"{{"code":"{code}"}}"#);
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            )
                        }
                        Err(status) => {
                            let body = r#"{"error":"boom"}"#;
                            format!(
                                "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            )
                        }
                    }
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        health_body.len(),
                        health_body
                    )
                };
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
            recorded_open_code_requests: recorded,
        }
    }
}

impl Drop for MockHealthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn make_entry(
    id: &str,
    name: &str,
    root: &str,
    branch: &str,
    port: u16,
    started_at: u64,
    doing: &str,
) -> RegistryEntry {
    RegistryEntry {
        id: id.to_string(),
        name: name.to_string(),
        root: PathBuf::from(root),
        branch: Some(branch.to_string()),
        pid: std::process::id(),
        port,
        started_at,
        doing: doing.to_string(),
        state: SurfaceState::Idle,
    }
}

#[test]
fn run_surfaces_with_no_entries_prints_empty_message() {
    let tmp = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_surfaces(tmp.path(), false, &mut out, &mut err);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out).unwrap(), "no surfaces are running\n");
    assert!(err.is_empty());
}

#[test]
fn run_surfaces_prints_aligned_table_in_started_at_order() {
    let tmp = tempfile::tempdir().unwrap();
    let server_a = MockHealthServer::start("s00000000000000aa");
    let server_b = MockHealthServer::start("s00000000000000bb");

    let mut later = make_entry(
        "s00000000000000bb",
        "payments-api",
        "/tmp/work/payments",
        "feat/webhooks",
        server_b.port,
        200,
        "add retry backoff",
    );
    later.state = SurfaceState::Working;
    let earlier = make_entry(
        "s00000000000000aa",
        "web",
        "/tmp/work/frontend",
        "main",
        server_a.port,
        100,
        "idle",
    );
    write_entry(tmp.path(), &later).unwrap();
    write_entry(tmp.path(), &earlier).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_surfaces(tmp.path(), false, &mut out, &mut err);
    assert_eq!(code, 0);
    assert!(err.is_empty());

    let stdout = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert_eq!(
        lines[0],
        "NAME          REPOSITORY  BRANCH         STATE    DOING"
    );
    assert_eq!(
        lines[1],
        "web           frontend    main           idle     idle"
    );
    assert_eq!(
        lines[2],
        "payments-api  payments    feat/webhooks  working  add retry backoff"
    );
}

#[test]
fn run_surfaces_table_prints_all_four_state_wordings() {
    let tmp = tempfile::tempdir().unwrap();
    let s1 = MockHealthServer::start("s00000000000000a1");
    let s2 = MockHealthServer::start("s00000000000000a2");
    let s3 = MockHealthServer::start("s00000000000000a3");
    let s4 = MockHealthServer::start("s00000000000000a4");

    let mut e1 = make_entry(
        "s00000000000000a1",
        "one",
        "/tmp/r1",
        "main",
        s1.port,
        100,
        "run deploy",
    );
    e1.state = SurfaceState::Approval;
    let mut e2 = make_entry(
        "s00000000000000a2",
        "two",
        "/tmp/r2",
        "main",
        s2.port,
        200,
        "idle",
    );
    e2.state = SurfaceState::Replied;
    let mut e3 = make_entry(
        "s00000000000000a3",
        "three",
        "/tmp/r3",
        "main",
        s3.port,
        300,
        "fix bug",
    );
    e3.state = SurfaceState::Working;
    let mut e4 = make_entry(
        "s00000000000000a4",
        "four",
        "/tmp/r4",
        "main",
        s4.port,
        400,
        "idle",
    );
    e4.state = SurfaceState::Idle;

    for e in [&e1, &e2, &e3, &e4] {
        write_entry(tmp.path(), e).unwrap();
    }

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_surfaces(tmp.path(), false, &mut out, &mut err);
    assert_eq!(code, 0);
    assert!(err.is_empty());

    let stdout = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 5, "{stdout}");
    assert_eq!(
        lines[0],
        "NAME   REPOSITORY  BRANCH  STATE                      DOING"
    );
    assert_eq!(
        lines[1],
        "one    r1          main    waiting on you (approval)  run deploy"
    );
    assert_eq!(
        lines[2],
        "two    r2          main    waiting on you (replied)   idle"
    );
    assert_eq!(
        lines[3],
        "three  r3          main    working                    fix bug"
    );
    assert_eq!(
        lines[4],
        "four   r4          main    idle                       idle"
    );
}

#[test]
fn run_surfaces_json_outputs_entries_and_empty_array() {
    let tmp = tempfile::tempdir().unwrap();

    let mut out_empty = Vec::new();
    let mut err_empty = Vec::new();
    let code_empty = run_surfaces(tmp.path(), true, &mut out_empty, &mut err_empty);
    assert_eq!(code_empty, 0);
    assert!(err_empty.is_empty());
    assert_eq!(String::from_utf8(out_empty).unwrap().trim(), "[]");

    let server_a = MockHealthServer::start("s00000000000000aa");
    let server_b = MockHealthServer::start("s00000000000000bb");
    let first = make_entry(
        "s00000000000000aa",
        "alpha",
        "/tmp/repo-a",
        "main",
        server_a.port,
        100,
        "idle",
    );
    let second = make_entry(
        "s00000000000000bb",
        "beta",
        "/tmp/repo-b",
        "dev",
        server_b.port,
        200,
        "refactor parser",
    );
    write_entry(tmp.path(), &second).unwrap();
    write_entry(tmp.path(), &first).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_surfaces(tmp.path(), true, &mut out, &mut err);
    assert_eq!(code, 0);
    assert!(err.is_empty());
    let parsed: Vec<RegistryEntry> = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed, vec![first, second]);
}

#[test]
fn run_open_posts_for_open_code_with_token_header_and_opens_open_url() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());

    let dir = tempfile::tempdir().unwrap();
    let server = MockHealthServer::start_with_open_code(
        "s00000000000000aa",
        Ok("one-time-code-777".to_string()),
    );
    let entry = make_entry(
        "s00000000000000aa",
        "alpha",
        "/tmp/repo-a",
        "main",
        server.port,
        100,
        "idle",
    );
    write_entry(dir.path(), &entry).unwrap();

    let token = "secret-token-xyz-9876";
    let token_path = surface_token_path_for_id(&entry.id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();

    let opened: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let opener = |url: &str| opened.borrow_mut().push(url.to_string());
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = run_open(dir.path(), "alpha", &opener, &mut out, &mut err);
    assert_eq!(code, 0);
    let expected_url = format!("http://127.0.0.1:{}/#open=one-time-code-777", server.port);
    assert_eq!(*opened.borrow(), vec![expected_url]);
    assert!(!opened.borrow()[0].contains(token));

    let reqs = server.recorded_open_code_requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert!(
        reqs[0].contains(&format!("X-Blyte-Token: {token}")),
        "expected token header in request: {}",
        reqs[0]
    );

    let stdout = String::from_utf8(out).unwrap();
    let stderr = String::from_utf8(err).unwrap();
    assert_eq!(stdout, "blyte: opened surface alpha\n");
    assert!(stderr.is_empty());
    assert!(!stdout.contains(token));
    assert!(!stderr.contains(token));
}

#[test]
fn run_open_fails_with_surface_name_and_opens_nothing_when_server_returns_500() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());

    let dir = tempfile::tempdir().unwrap();
    let server = MockHealthServer::start_with_open_code("s00000000000000aa", Err(500));
    let entry = make_entry(
        "s00000000000000aa",
        "payments",
        "/tmp/repo-a",
        "main",
        server.port,
        100,
        "idle",
    );
    write_entry(dir.path(), &entry).unwrap();

    let token = "secret-token-xyz-9876";
    let token_path = surface_token_path_for_id(&entry.id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();

    let opened: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let opener = |url: &str| opened.borrow_mut().push(url.to_string());
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = run_open(dir.path(), "payments", &opener, &mut out, &mut err);
    assert_eq!(code, 1);
    assert!(opened.borrow().is_empty());
    assert!(out.is_empty());
    let stderr = String::from_utf8(err).unwrap();
    assert!(stderr.contains("payments"), "stderr: {stderr}");
    assert!(!stderr.contains(token), "stderr must not leak token");
}

#[test]
fn run_open_unknown_name_returns_1_without_calling_opener() {
    let dir = tempfile::tempdir().unwrap();
    let opened: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let opener = |url: &str| opened.borrow_mut().push(url.to_string());
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = run_open(dir.path(), "ghost", &opener, &mut out, &mut err);
    assert_eq!(code, 1);
    assert!(opened.borrow().is_empty());
    assert!(out.is_empty());
    let stderr = String::from_utf8(err).unwrap();
    assert!(
        stderr.contains("no running surface named ghost"),
        "{stderr}"
    );
}

#[test]
fn run_open_missing_or_unusable_token_returns_1_without_calling_opener() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());

    let dir = tempfile::tempdir().unwrap();
    let server = MockHealthServer::start("s00000000000000aa");
    let entry = make_entry(
        "s00000000000000aa",
        "alpha",
        "/tmp/repo-a",
        "main",
        server.port,
        100,
        "idle",
    );
    write_entry(dir.path(), &entry).unwrap();

    let opened: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let opener = |url: &str| opened.borrow_mut().push(url.to_string());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_open(dir.path(), "alpha", &opener, &mut out, &mut err);
    assert_eq!(code, 1);
    assert!(opened.borrow().is_empty());
    assert!(out.is_empty());
    let stderr = String::from_utf8(err).unwrap();
    assert!(stderr.contains("has no usable token"), "{stderr}");

    let token_path = surface_token_path_for_id(&entry.id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, "   \n  ").unwrap();
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_open(dir.path(), "alpha", &opener, &mut out2, &mut err2);
    assert_eq!(code2, 1);
    assert!(opened.borrow().is_empty());
    let stderr2 = String::from_utf8(err2).unwrap();
    assert!(stderr2.contains("has no usable token"), "{stderr2}");

    std::fs::write(&token_path, "line1\nline2\n").unwrap();
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_open(dir.path(), "alpha", &opener, &mut out3, &mut err3);
    assert_eq!(code3, 1);
    assert!(opened.borrow().is_empty());
    let stderr3 = String::from_utf8(err3).unwrap();
    assert!(stderr3.contains("has no usable token"), "{stderr3}");
}

#[test]
fn blyte_surfaces_binary_with_empty_state_dir_prints_no_surfaces() {
    let state = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("surfaces")
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "no surfaces are running\n"
    );
}

#[test]
fn blyte_help_lists_surfaces_and_open() {
    let output = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("surfaces"), "{help}");
    assert!(
        help.contains("List the surfaces running on this machine"),
        "{help}"
    );
    assert!(help.contains("open"), "{help}");
    assert!(
        help.contains("Open a running surface's page by name"),
        "{help}"
    );
}
