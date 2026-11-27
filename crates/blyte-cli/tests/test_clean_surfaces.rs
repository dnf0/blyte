use blyte_agent::sandbox::{surface_token_path, surface_token_path_for_id};
use blyte_cli::clean::clean_surfaces;
use blyte_cli::surface_dir::{self, SurfaceRecord};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct XdgStateGuard {
    _lock: MutexGuard<'static, ()>,
    _dir: TempDir,
    prev: Option<OsString>,
}

impl XdgStateGuard {
    fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir for XDG_STATE_HOME");
        let prev = std::env::var_os("XDG_STATE_HOME");
        std::env::set_var("XDG_STATE_HOME", dir.path());
        Self {
            _lock: lock,
            _dir: dir,
            prev,
        }
    }

    fn path(&self) -> &Path {
        self._dir.path()
    }
}

impl Drop for XdgStateGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }
}

fn init_temp_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir for repo");
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test User"]);
    std::fs::write(tmp.path().join("README.md"), "hello\n").expect("write README");
    run(&["add", "README.md"]);
    run(&["commit", "-m", "init"]);
    tmp
}

fn unused_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn assert_health_request(req: &str) {
    let request_line = req.lines().next().unwrap_or("");
    assert!(
        request_line == "GET /api/health HTTP/1.1" || request_line == "GET /api/health HTTP/1.0",
        "unexpected health request line: {request_line:?}"
    );
}

fn spawn_health_listener(body: String) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind health listener");
    let port = listener.local_addr().expect("local_addr").port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept health probe");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let req = read_http_request(&mut stream);
        assert_health_request(&req);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(resp.as_bytes())
            .expect("write health response");
        let _ = stream.flush();
    });
    (port, handle)
}

fn spawn_held_health_listener(
    raw_response: String,
) -> (u16, mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind held health listener");
    let port = listener.local_addr().expect("local_addr").port();
    let (release_tx, release_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept held health probe");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let req = read_http_request(&mut stream);
        assert_health_request(&req);
        stream
            .write_all(raw_response.as_bytes())
            .expect("write raw health response");
        stream.flush().expect("flush raw health response");
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
        drop(stream);
    });
    (port, release_tx, handle)
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, content).expect("write file");
}

fn set_mtime(path: &Path, unix_secs: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file for set_modified");
    let ts = UNIX_EPOCH + Duration::from_secs(unix_secs);
    file.set_modified(ts).expect("set_modified");
}

fn blyte_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_blyte"))
}

const NOW_SECS: u64 = 1_800_000_000;
const DAY_SECS: u64 = 86_400;

#[test]
fn old_stopped_surface_files_and_token_are_removed() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let port = unused_port();
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");

    write_file(
        &surface_file,
        &format!(r#"{{"pid":99999,"port":{port},"session_id":"old-sess-1","started_at":100}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "secret-token\n");

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    assert_eq!(lines.len(), 3, "expected 3 lines, got: {lines:?}");
    assert!(!surface_file.exists(), ".wt/surface.json should be removed");
    assert!(
        !repo.path().join(".wt/sessions").exists(),
        ".wt/sessions/ should be removed"
    );
    assert!(!token_path.exists(), "old token file should be removed");
}

#[test]
fn old_running_surface_is_left_alone() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let (port, handle) = spawn_health_listener(r#"{"session_id":"live-sess-1"}"#.to_string());
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");

    write_file(
        &surface_file,
        &format!(r#"{{"pid":99999,"port":{port},"session_id":"live-sess-1","started_at":100}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "secret-token\n");

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    handle.join().expect("listener thread");

    assert!(lines.is_empty(), "expected no lines, got: {lines:?}");
    assert!(surface_file.exists(), ".wt/surface.json must stay");
    assert!(sessions_file.exists(), ".wt/sessions/ must stay");
    assert!(token_path.exists(), "old token file must stay");
}

#[test]
fn old_surface_with_mismatched_session_id_is_removed() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let (port, handle) = spawn_health_listener(r#"{"session_id":"other-sess"}"#.to_string());
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");

    write_file(
        &surface_file,
        &format!(r#"{{"pid":99999,"port":{port},"session_id":"expected-sess","started_at":100}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "secret-token\n");

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    handle.join().expect("listener thread");

    assert_eq!(lines.len(), 3, "expected 3 lines, got: {lines:?}");
    assert!(!surface_file.exists(), ".wt/surface.json should be removed");
    assert!(
        !repo.path().join(".wt/sessions").exists(),
        ".wt/sessions/ should be removed"
    );
    assert!(!token_path.exists(), "old token file should be removed");
}

#[test]
fn old_running_surface_chunked_response_is_left_alone() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let body = r#"{"session_id":"live-chunked"}"#;
    let raw_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
        body.len(),
        body
    );
    let (port, release_tx, handle) = spawn_held_health_listener(raw_response);
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");

    write_file(
        &surface_file,
        &format!(r#"{{"pid":99999,"port":{port},"session_id":"live-chunked","started_at":100}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "secret-token\n");

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    let _ = release_tx.send(());
    handle.join().expect("listener thread");

    assert!(lines.is_empty(), "expected no lines, got: {lines:?}");
    assert!(surface_file.exists(), ".wt/surface.json must stay");
    assert!(sessions_file.exists(), ".wt/sessions/ must stay");
    assert!(token_path.exists(), "old token file must stay");
}

#[test]
fn old_running_surface_keepalive_connection_is_left_alone() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let body = r#"{"session_id":"live-keepalive"}"#;
    let raw_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
        body.len(),
        body
    );
    let (port, release_tx, handle) = spawn_held_health_listener(raw_response);
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");

    write_file(
        &surface_file,
        &format!(r#"{{"pid":99999,"port":{port},"session_id":"live-keepalive","started_at":100}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "secret-token\n");

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    let _ = release_tx.send(());
    handle.join().expect("listener thread");

    assert!(lines.is_empty(), "expected no lines, got: {lines:?}");
    assert!(surface_file.exists(), ".wt/surface.json must stay");
    assert!(sessions_file.exists(), ".wt/sessions/ must stay");
    assert!(token_path.exists(), "old token file must stay");
}

#[test]
fn dry_run_reports_old_surface_removals_without_deleting() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let port = unused_port();
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");

    write_file(
        &surface_file,
        &format!(r#"{{"port":{port},"session_id":"old-sess-dry"}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "secret-token\n");

    let lines = clean_surfaces(repo.path(), 14, true, NOW_SECS).expect("clean_surfaces dry_run");
    assert_eq!(lines.len(), 3, "expected 3 lines, got: {lines:?}");
    for line in &lines {
        assert!(
            line.starts_with("would remove "),
            "expected dry-run line to start with 'would remove ': {line}"
        );
    }
    assert!(surface_file.exists(), ".wt/surface.json must remain");
    assert!(sessions_file.exists(), ".wt/sessions/ must remain");
    assert!(token_path.exists(), "old token file must remain");
}

#[test]
fn old_sessions_removed_even_without_surface_json() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    write_file(&sessions_file, "{\"type\":\"note\"}\n");

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    assert_eq!(lines.len(), 1, "expected 1 line, got: {lines:?}");
    assert!(
        !repo.path().join(".wt/sessions").exists(),
        ".wt/sessions/ should be removed"
    );
}

#[test]
fn stopped_surface_older_than_threshold_and_token_are_removed() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let id = "s0123456789abcdef";
    let record = SurfaceRecord {
        id: id.to_string(),
        name: "main".to_string(),
        pid: 99999,
        port: unused_port(),
        session_id: "sess-15d".to_string(),
        started_at: NOW_SECS - DAY_SECS,
    };
    let sdir = surface_dir::surface_dir(repo.path(), id);
    surface_dir::write_record_in(&sdir, &record).expect("write record");
    let entries = sdir.join("entries.jsonl");
    write_file(&entries, "{\"type\":\"note\"}\n");
    let token_path = surface_token_path_for_id(id).expect("token path for id");
    write_file(&token_path, "per-surface-token\n");

    set_mtime(&sdir.join("surface.json"), NOW_SECS - 16 * DAY_SECS);
    set_mtime(&entries, NOW_SECS - 15 * DAY_SECS);

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    assert_eq!(lines.len(), 2, "expected 2 lines, got: {lines:?}");
    assert!(
        !surface_dir::surface_dir(repo.path(), id).exists(),
        "stopped surface directory should be removed"
    );
    assert!(
        !token_path.exists(),
        "stopped surface token should be removed"
    );
}

#[test]
fn stopped_surface_younger_than_threshold_is_kept_with_token() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let id = "s0123456789abcdef";
    let record = SurfaceRecord {
        id: id.to_string(),
        name: "main".to_string(),
        pid: 99999,
        port: unused_port(),
        session_id: "sess-13d".to_string(),
        started_at: NOW_SECS - 13 * DAY_SECS,
    };
    let sdir = surface_dir::surface_dir(repo.path(), id);
    surface_dir::write_record_in(&sdir, &record).expect("write record");
    let entries = sdir.join("entries.jsonl");
    write_file(&entries, "{\"type\":\"note\"}\n");
    let token_path = surface_token_path_for_id(id).expect("token path for id");
    write_file(&token_path, "per-surface-token\n");

    let recent_ts = NOW_SECS - 13 * DAY_SECS;
    set_mtime(&sdir.join("surface.json"), recent_ts);
    set_mtime(&entries, recent_ts);

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    assert!(lines.is_empty(), "expected no lines, got: {lines:?}");
    assert!(
        surface_dir::surface_dir(repo.path(), id).exists(),
        "young stopped surface directory must stay"
    );
    assert!(token_path.exists(), "young stopped surface token must stay");
}

#[test]
fn stopped_surface_with_old_record_and_newer_file_is_kept() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let id = "s0123456789abcdef";
    let record = SurfaceRecord {
        id: id.to_string(),
        name: "main".to_string(),
        pid: 99999,
        port: unused_port(),
        session_id: "sess-mixed".to_string(),
        started_at: NOW_SECS - 20 * DAY_SECS,
    };
    let sdir = surface_dir::surface_dir(repo.path(), id);
    surface_dir::write_record_in(&sdir, &record).expect("write record");
    let entries = sdir.join("entries.jsonl");
    write_file(&entries, "{\"type\":\"note\"}\n");
    let token_path = surface_token_path_for_id(id).expect("token path for id");
    write_file(&token_path, "per-surface-token\n");

    set_mtime(&sdir.join("surface.json"), NOW_SECS - 15 * DAY_SECS);
    set_mtime(&entries, NOW_SECS - 13 * DAY_SECS);

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    assert!(lines.is_empty(), "expected no lines, got: {lines:?}");
    assert!(
        surface_dir::surface_dir(repo.path(), id).exists(),
        "surface directory with a newer file must stay"
    );
    assert!(
        token_path.exists(),
        "token for surface directory with a newer file must stay"
    );
}

#[test]
fn running_surface_with_old_files_is_kept() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let id = "s0123456789abcdef";
    let (port, handle) = spawn_health_listener(format!(
        r#"{{"surface_id":"{id}","session_id":"sess-live"}}"#
    ));
    let record = SurfaceRecord {
        id: id.to_string(),
        name: "main".to_string(),
        pid: std::process::id(),
        port,
        session_id: "sess-live".to_string(),
        started_at: NOW_SECS - 15 * DAY_SECS,
    };
    let sdir = surface_dir::surface_dir(repo.path(), id);
    surface_dir::write_record_in(&sdir, &record).expect("write record");
    let entries = sdir.join("entries.jsonl");
    write_file(&entries, "{\"type\":\"note\"}\n");
    let token_path = surface_token_path_for_id(id).expect("token path for id");
    write_file(&token_path, "per-surface-token\n");

    let old_ts = NOW_SECS - 15 * DAY_SECS;
    set_mtime(&sdir.join("surface.json"), old_ts);
    set_mtime(&entries, old_ts);

    let lines = clean_surfaces(repo.path(), 14, false, NOW_SECS).expect("clean_surfaces ok");
    handle.join().expect("listener thread");

    assert!(lines.is_empty(), "expected no lines, got: {lines:?}");
    assert!(
        surface_dir::surface_dir(repo.path(), id).exists(),
        "running surface directory must stay"
    );
    assert!(token_path.exists(), "running surface token must stay");
}

#[test]
fn surface_dir_missing_record_returns_error_and_stays() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let id = "s0123456789abcdef";
    let broken_dir = surface_dir::surface_dir(repo.path(), id);
    std::fs::create_dir_all(&broken_dir).expect("create broken surface dir");

    let err = clean_surfaces(repo.path(), 14, false, NOW_SECS)
        .expect_err("expected error for missing surface.json");
    assert!(
        err.contains(id),
        "error should name the broken surface directory ({id}): {err}"
    );
    assert!(
        broken_dir.exists(),
        "directory without surface.json must never be removed"
    );
}

#[test]
fn unparsable_old_surface_json_returns_error_and_leaves_sessions() {
    let _xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    write_file(&surface_file, "{not valid json");
    write_file(&sessions_file, "{\"type\":\"note\"}\n");

    let err = clean_surfaces(repo.path(), 14, false, NOW_SECS)
        .expect_err("expected error for unparsable surface.json");
    assert!(
        err.contains("surface.json"),
        "error should name surface.json: {err}"
    );
    assert!(surface_file.exists(), "unparsable surface.json must stay");
    assert!(sessions_file.exists(), ".wt/sessions/ must stay");
}

#[test]
fn cli_blyte_clean_removes_old_surface_and_exits_nonzero_on_error() {
    let xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let port = unused_port();
    let surface_file = repo.path().join(".wt/surface.json");
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    let token_path = surface_token_path(repo.path()).expect("token path");
    write_file(
        &surface_file,
        &format!(r#"{{"port":{port},"session_id":"cli-old"}}"#),
    );
    write_file(&sessions_file, "{\"type\":\"note\"}\n");
    write_file(&token_path, "tok\n");

    let out = Command::new(blyte_bin())
        .arg("clean")
        .current_dir(repo.path())
        .env("XDG_STATE_HOME", xdg.path())
        .output()
        .expect("run blyte clean");
    assert!(
        out.status.success(),
        "blyte clean failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("removed .wt/surface.json"),
        "stdout missing surface.json removal: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/sessions/"),
        "stdout missing sessions removal: {stdout}"
    );
    assert!(!surface_file.exists());
    assert!(!repo.path().join(".wt/sessions").exists());
    assert!(!token_path.exists());

    let broken_dir = surface_dir::surface_dir(repo.path(), "s0123456789abcdef");
    std::fs::create_dir_all(&broken_dir).expect("create broken dir");
    let out_err = Command::new(blyte_bin())
        .arg("clean")
        .current_dir(repo.path())
        .env("XDG_STATE_HOME", xdg.path())
        .output()
        .expect("run blyte clean with broken dir");
    assert!(
        !out_err.status.success(),
        "blyte clean should exit non-zero on broken surface dir"
    );
    let stderr = String::from_utf8_lossy(&out_err.stderr);
    assert!(
        stderr.contains("blyte: "),
        "stderr should prefix error with 'blyte: ': {stderr}"
    );
    assert!(
        stderr.contains("s0123456789abcdef"),
        "stderr should name broken surface dir: {stderr}"
    );
    assert!(broken_dir.exists());
}

#[test]
fn cli_blyte_clean_quiet_suppresses_surface_removal_lines() {
    let xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let sessions_file = repo.path().join(".wt/sessions/entries.jsonl");
    write_file(&sessions_file, "{\"type\":\"note\"}\n");

    let out = Command::new(blyte_bin())
        .args(["clean", "--quiet"])
        .current_dir(repo.path())
        .env("XDG_STATE_HOME", xdg.path())
        .output()
        .expect("run blyte clean --quiet");
    assert!(
        out.status.success(),
        "blyte clean --quiet failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "quiet clean should print nothing to stdout"
    );
    assert!(!repo.path().join(".wt/sessions").exists());
}

#[test]
fn cli_blyte_clean_dry_run_leaves_stopped_surface_intact() {
    let xdg = XdgStateGuard::new();
    let repo = init_temp_repo();
    let id = "s0123456789abcdef";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now")
        .as_secs();
    let old_ts = now - 20 * DAY_SECS;
    let record = SurfaceRecord {
        id: id.to_string(),
        name: "main".to_string(),
        pid: 99999,
        port: unused_port(),
        session_id: "sess-cli-dry".to_string(),
        started_at: old_ts,
    };
    let sdir = surface_dir::surface_dir(repo.path(), id);
    surface_dir::write_record_in(&sdir, &record).expect("write record");
    set_mtime(&sdir.join("surface.json"), old_ts);

    let out = Command::new(blyte_bin())
        .args(["clean", "--dry-run"])
        .current_dir(repo.path())
        .env("XDG_STATE_HOME", xdg.path())
        .output()
        .expect("run blyte clean --dry-run");
    assert!(
        out.status.success(),
        "blyte clean --dry-run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("would remove .wt/surfaces/{id}")),
        "stdout missing dry-run surface removal: {stdout}"
    );
    assert!(surface_dir::surface_dir(repo.path(), id).exists());
}
