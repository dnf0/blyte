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
use blyte_cli::http::Request;
use blyte_cli::serve::{generate_token, route, Surface, TOKEN_HEADER};
use blyte_cli::session::{EntryBody, PromptRunner, Session};
use blyte_cli::surface_registry::{registry_dir, write_entry, RegistryEntry, SurfaceState};
use blyte_cli::surfaces_cmd::{surface_url, surfaces_payload};

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
}

impl MockHealthServer {
    fn start(surface_id: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let body = format!(r#"{{"surface_id":"{surface_id}"}}"#);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut req = Vec::new();
                let mut buf = [0u8; 512];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
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

struct NoopRunner;

impl PromptRunner for NoopRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        Ok(())
    }
}

fn make_entry(
    id: &str,
    name: &str,
    root: &Path,
    branch: Option<&str>,
    port: u16,
    started_at: u64,
    doing: &str,
) -> RegistryEntry {
    RegistryEntry {
        id: id.to_string(),
        name: name.to_string(),
        root: root.to_path_buf(),
        branch: branch.map(str::to_string),
        pid: std::process::id(),
        port,
        started_at,
        doing: doing.to_string(),
        state: SurfaceState::Idle,
    }
}

fn write_token(id: &str, token: &str) {
    let token_path = surface_token_path_for_id(id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();
}

fn make_surface(id: &str, name: &str, root: PathBuf) -> Surface {
    let session = Session::start(Box::new(NoopRunner), "gemini-4-neon".to_string());
    Surface {
        surface_id: id.to_string(),
        surface_name: name.to_string(),
        root,
        repo: name.to_string(),
        session,
        session_id: "sess-switcher-test".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        message_credential: generate_token().unwrap(),
        requests: Arc::new(blyte_cli::requests::RequestsState::new()),
        publisher: Arc::new(blyte_cli::requests::RealPublisher),
        reconcile_lock: Mutex::new(()),
        reconcile_requested: AtomicBool::new(false),
    }
}

fn make_request(method: &str, path: &str, token: Option<&str>) -> Request {
    let headers = match token {
        Some(t) => vec![(TOKEN_HEADER.to_string(), t.to_string())],
        None => Vec::new(),
    };
    Request {
        method: method.to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers,
        body: Vec::new(),
    }
}

#[test]
fn surfaces_payload_lists_two_live_entries_sorted_by_name_with_urls_and_current() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let reg = tempfile::tempdir().unwrap();
    let roots = tempfile::tempdir().unwrap();
    let root_a = roots.path().join("alpha-repo");
    let root_z = roots.path().join("zeta-repo");

    let srv_z = MockHealthServer::start("s00000000000000bb");
    let srv_a = MockHealthServer::start("s00000000000000aa");

    let mut entry_z = make_entry(
        "s00000000000000bb",
        "zeta",
        &root_z,
        None,
        srv_z.port,
        100,
        "fix billing retries",
    );
    entry_z.state = SurfaceState::Working;
    let entry_a = make_entry(
        "s00000000000000aa",
        "alpha",
        &root_a,
        Some("main"),
        srv_a.port,
        200,
        "idle",
    );
    write_entry(reg.path(), &entry_z).unwrap();
    write_entry(reg.path(), &entry_a).unwrap();
    write_token(&entry_a.id, "tok-alpha-111");
    write_token(&entry_z.id, "tok-zeta-222");

    assert_eq!(
        surface_url(&entry_a).unwrap(),
        format!("http://127.0.0.1:{}/#token=tok-alpha-111", srv_a.port)
    );

    let payload = surfaces_payload(reg.path(), &entry_z.id, &[]).unwrap();
    let list = payload["surfaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);

    assert_eq!(list[0]["id"], "s00000000000000aa");
    assert_eq!(list[0]["name"], "alpha");
    assert_eq!(list[0]["root"], root_a.to_string_lossy().as_ref());
    assert_eq!(list[0]["branch"], "main");
    assert_eq!(list[0]["doing"], "idle");
    assert_eq!(list[0]["state"], "idle");
    assert_eq!(list[0]["current"], false);
    assert_eq!(
        list[0]["url"],
        format!("http://127.0.0.1:{}/#token=tok-alpha-111", srv_a.port)
    );
    assert!(list[0]["error"].is_null());

    assert_eq!(list[1]["id"], "s00000000000000bb");
    assert_eq!(list[1]["name"], "zeta");
    assert_eq!(list[1]["root"], root_z.to_string_lossy().as_ref());
    assert!(list[1]["branch"].is_null());
    assert_eq!(list[1]["doing"], "fix billing retries");
    assert_eq!(list[1]["state"], "working");
    assert_eq!(list[1]["current"], true);
    assert_eq!(
        list[1]["url"],
        format!("http://127.0.0.1:{}/#token=tok-zeta-222", srv_z.port)
    );
    assert!(list[1]["error"].is_null());
}

#[test]
fn surfaces_payload_breaks_name_ties_by_started_at() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let reg = tempfile::tempdir().unwrap();
    let roots = tempfile::tempdir().unwrap();

    let srv_old = MockHealthServer::start("s0000000000000011");
    let srv_new = MockHealthServer::start("s0000000000000022");

    let newer = make_entry(
        "s0000000000000022",
        "shared",
        &roots.path().join("r2"),
        Some("feat/b"),
        srv_new.port,
        500,
        "second",
    );
    let older = make_entry(
        "s0000000000000011",
        "shared",
        &roots.path().join("r1"),
        Some("feat/a"),
        srv_old.port,
        100,
        "first",
    );
    write_entry(reg.path(), &newer).unwrap();
    write_entry(reg.path(), &older).unwrap();
    write_token(&older.id, "tok-old");
    write_token(&newer.id, "tok-new");

    let payload = surfaces_payload(reg.path(), &older.id, &[]).unwrap();
    let list = payload["surfaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "s0000000000000011");
    assert_eq!(list[1]["id"], "s0000000000000022");
}

#[test]
fn surfaces_payload_lists_missing_token_entry_with_null_url_and_error() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let reg = tempfile::tempdir().unwrap();
    let roots = tempfile::tempdir().unwrap();

    let srv_ok = MockHealthServer::start("s00000000000000aa");
    let srv_bad = MockHealthServer::start("s00000000000000bb");

    let ok_entry = make_entry(
        "s00000000000000aa",
        "alpha",
        &roots.path().join("a"),
        Some("main"),
        srv_ok.port,
        100,
        "idle",
    );
    let missing_token_entry = make_entry(
        "s00000000000000bb",
        "beta",
        &roots.path().join("b"),
        Some("dev"),
        srv_bad.port,
        200,
        "investigating",
    );
    write_entry(reg.path(), &ok_entry).unwrap();
    write_entry(reg.path(), &missing_token_entry).unwrap();
    write_token(&ok_entry.id, "tok-ok");

    let payload = surfaces_payload(reg.path(), &ok_entry.id, &[]).unwrap();
    let list = payload["surfaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);

    assert_eq!(list[1]["id"], "s00000000000000bb");
    assert!(list[1]["url"].is_null());
    let err = list[1]["error"].as_str().unwrap();
    assert!(!err.is_empty());
    assert!(err.contains("has no usable token"), "{err}");
}

#[test]
fn surfaces_payload_omits_dead_process_entries() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let reg = tempfile::tempdir().unwrap();
    let roots = tempfile::tempdir().unwrap();

    let srv_live = MockHealthServer::start("s00000000000000aa");
    let mut dead_child = Command::new("true").spawn().unwrap();
    let dead_pid = dead_child.id();
    let _ = dead_child.wait();

    let closed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed_listener.local_addr().unwrap().port();
    drop(closed_listener);

    let live_entry = make_entry(
        "s00000000000000aa",
        "alive",
        &roots.path().join("alive"),
        Some("main"),
        srv_live.port,
        100,
        "idle",
    );
    let mut dead_entry = make_entry(
        "s00000000000000dd",
        "dead",
        &roots.path().join("dead"),
        Some("main"),
        closed_port,
        50,
        "gone",
    );
    dead_entry.pid = dead_pid;
    write_entry(reg.path(), &live_entry).unwrap();
    write_entry(reg.path(), &dead_entry).unwrap();
    write_token(&live_entry.id, "tok-live");
    write_token(&dead_entry.id, "tok-dead");

    let payload = surfaces_payload(reg.path(), &live_entry.id, &[]).unwrap();
    let list = payload["surfaces"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], "s00000000000000aa");
}

#[test]
fn surfaces_payload_errors_when_current_id_is_absent() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let reg = tempfile::tempdir().unwrap();
    let roots = tempfile::tempdir().unwrap();

    let srv = MockHealthServer::start("s00000000000000aa");
    let entry = make_entry(
        "s00000000000000aa",
        "alpha",
        &roots.path().join("a"),
        Some("main"),
        srv.port,
        100,
        "idle",
    );
    write_entry(reg.path(), &entry).unwrap();
    write_token(&entry.id, "tok-alpha");

    let missing_id = "s00000000000000ff";
    let err = surfaces_payload(reg.path(), missing_id, &[]).unwrap_err();
    assert!(err.contains(missing_id), "{err}");
}

#[test]
fn api_surfaces_route_checks_token_returns_payload_and_errors_on_missing_current() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let roots = tempfile::tempdir().unwrap();
    let root_a = roots.path().join("alpha");
    std::fs::create_dir_all(&root_a).unwrap();
    let root_b = roots.path().join("beta");

    let surface = make_surface("s00000000000000aa", "alpha", root_a.clone());

    let unauth = route(&surface, &make_request("GET", "/api/surfaces", None), 0);
    assert_eq!(unauth.status, 403);

    let wrong = route(
        &surface,
        &make_request("GET", "/api/surfaces", Some("wrong-token")),
        0,
    );
    assert_eq!(wrong.status, 403);

    let post_resp = route(
        &surface,
        &make_request("POST", "/api/surfaces", Some(&surface.token)),
        0,
    );
    assert_eq!(post_resp.status, 405);

    let missing_resp = route(
        &surface,
        &make_request("GET", "/api/surfaces", Some(&surface.token)),
        0,
    );
    assert_eq!(missing_resp.status, 500);
    let missing_body: serde_json::Value = serde_json::from_slice(&missing_resp.body).unwrap();
    assert!(
        missing_body["error"]
            .as_str()
            .unwrap()
            .contains(&surface.surface_id),
        "{missing_body}"
    );

    let srv_a = MockHealthServer::start("s00000000000000aa");
    let srv_b = MockHealthServer::start("s00000000000000bb");
    let reg = registry_dir().unwrap();
    write_entry(
        &reg,
        &make_entry(
            "s00000000000000aa",
            "alpha",
            &root_a,
            Some("main"),
            srv_a.port,
            100,
            "idle",
        ),
    )
    .unwrap();
    write_entry(
        &reg,
        &make_entry(
            "s00000000000000bb",
            "beta",
            &root_b,
            Some("feat/x"),
            srv_b.port,
            200,
            "working",
        ),
    )
    .unwrap();
    write_token("s00000000000000aa", &surface.token);
    write_token("s00000000000000bb", "tok-beta-secret");

    let before_seq = surface.session.info().last_seq;
    let ok_resp = route(
        &surface,
        &make_request("GET", "/api/surfaces", Some(&surface.token)),
        0,
    );
    assert_eq!(ok_resp.status, 200);
    assert_eq!(surface.session.info().last_seq, before_seq);

    let body: serde_json::Value = serde_json::from_slice(&ok_resp.body).unwrap();
    let list = body["surfaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "s00000000000000aa");
    assert_eq!(list[0]["current"], true);
    assert_eq!(list[1]["id"], "s00000000000000bb");
    assert_eq!(list[1]["current"], false);
    assert_eq!(
        list[1]["url"],
        format!("http://127.0.0.1:{}/#token=tok-beta-secret", srv_b.port)
    );
}

#[test]
fn surfaces_payload_carries_all_four_states() {
    let state = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set_state_root(state.path());
    let reg = tempfile::tempdir().unwrap();
    let roots = tempfile::tempdir().unwrap();

    let s1 = MockHealthServer::start("s00000000000000a1");
    let s2 = MockHealthServer::start("s00000000000000a2");
    let s3 = MockHealthServer::start("s00000000000000a3");
    let s4 = MockHealthServer::start("s00000000000000a4");

    let mut e1 = make_entry(
        "s00000000000000a1",
        "a-idle",
        &roots.path().join("r1"),
        Some("main"),
        s1.port,
        100,
        "idle",
    );
    e1.state = SurfaceState::Idle;
    let mut e2 = make_entry(
        "s00000000000000a2",
        "b-working",
        &roots.path().join("r2"),
        Some("main"),
        s2.port,
        200,
        "work",
    );
    e2.state = SurfaceState::Working;
    let mut e3 = make_entry(
        "s00000000000000a3",
        "c-approval",
        &roots.path().join("r3"),
        Some("main"),
        s3.port,
        300,
        "deploy",
    );
    e3.state = SurfaceState::Approval;
    let mut e4 = make_entry(
        "s00000000000000a4",
        "d-replied",
        &roots.path().join("r4"),
        Some("main"),
        s4.port,
        400,
        "idle",
    );
    e4.state = SurfaceState::Replied;

    for e in [&e1, &e2, &e3, &e4] {
        write_entry(reg.path(), e).unwrap();
        write_token(&e.id, &format!("tok-{}", e.name));
    }

    let payload = surfaces_payload(reg.path(), &e1.id, &[]).unwrap();
    let list = payload["surfaces"].as_array().unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(list[0]["state"], "idle");
    assert_eq!(list[1]["state"], "working");
    assert_eq!(list[2]["state"], "approval");
    assert_eq!(list[3]["state"], "replied");
}
