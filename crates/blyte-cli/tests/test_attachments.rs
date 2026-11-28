use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{BaseCheck, Budget, Runner};
use blyte_agent::sandbox;
use blyte_agent::shell::{LocalShell, Output, Shell};
use blyte_agent::state::State;
use blyte_cli::attachments::{
    attachments_dir, base64_encode, detect_image, load, store, MAX_ATTACHMENT_BYTES,
};
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{run_with_runner, run_with_runner_factory};
use blyte_cli::session::{AgentRunner, Entry, EntryBody, Paste, PromptRunner, Session};
use blyte_cli::task_runner::{build_local_shell, build_sandbox_profile, TaskConfig};
use blyte_provider::{Message, Provider, ProviderError, Request, Response, UserPart};

const WAIT: Duration = Duration::from_secs(30);

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct StateDirGuard {
    _lock: MutexGuard<'static, ()>,
    prev_state: Option<OsString>,
    prev_roots: Vec<PathBuf>,
}

impl StateDirGuard {
    fn set(dir: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_state = std::env::var_os("XDG_STATE_HOME");
        let prev_roots = blyte_agent::tools::read_roots();
        std::env::set_var("XDG_STATE_HOME", dir);
        Self {
            _lock: lock,
            prev_state,
            prev_roots,
        }
    }
}

impl Drop for StateDirGuard {
    fn drop(&mut self) {
        match &self.prev_state {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        blyte_agent::tools::set_read_roots(self.prev_roots.clone());
        blyte_agent::tools::set_write_scope(None);
        blyte_agent::tools::set_approval(None);
    }
}

struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct RecordingProvider {
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Provider for RecordingProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response {
            text: "noted".into(),
            ..Default::default()
        })
    }

    fn model_id(&self) -> String {
        "recording".into()
    }
}

struct StubShell;

impl Shell for StubShell {
    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn describe(&self) -> String {
        "stub".into()
    }
}

struct ScriptedAgentRunner {
    provider: RecordingProvider,
    root: PathBuf,
    state: State,
}

impl ScriptedAgentRunner {
    fn new(root: PathBuf) -> (Self, Arc<Mutex<Vec<Request>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runner = Self {
            provider: RecordingProvider {
                seen: Arc::clone(&seen),
            },
            root,
            state: State::new("", ""),
        };
        (runner, seen)
    }
}

impl PromptRunner for ScriptedAgentRunner {
    fn set_pending_user_parts(&mut self, parts: Vec<UserPart>) {
        self.state.runtime.pending_user_parts = parts;
    }

    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.state.brief.goal = prompt.to_string();
        let runner = Runner {
            provider: &self.provider,
            shell: &StubShell,
            fs: &LocalFs,
            context: &Conversational,
            root: self.root.clone(),
            budget: Budget {
                max_turns: 1,
                max_gate_attempts: 1,
                max_duration: None,
            },
            rollback: false,
            on_event: None,
            research: false,
            claim_check: false,
            base_check: BaseCheck::None,
        };
        runner.run(&mut self.state);
        emit(EntryBody::Turn {
            n: 1,
            text: "noted".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
        });
        Ok(())
    }
}

struct RunningSurface {
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<i32>>,
}

impl Drop for RunningSurface {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-q", "--no-gpg-sign", "-m", "init"]);
}

fn default_repl_config(root: PathBuf) -> ReplConfig {
    ReplConfig {
        repo_root: root,
        converse: true,
        ..Default::default()
    }
}

fn default_surface_args() -> SurfaceArgs {
    SurfaceArgs {
        no_open: true,
        name: Some("test-surface".to_string()),
        ..SurfaceArgs::default()
    }
}

fn parse_surface_ready_line(line: &str) -> Option<(String, u16, String)> {
    let rest = line.trim().strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at ")?;
    let after_scheme = url.trim().strip_prefix("http://127.0.0.1:")?;
    let (port_str, token) = after_scheme.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    Some((name.to_string(), port, token.to_string()))
}

fn start_surface(root: &Path, runner: Box<dyn PromptRunner>) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let root_bg = root.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let args = default_surface_args();
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(root_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_runner(&args, cfg, &mut out, &mut err, &mut opener, stop_bg, runner)
    });

    let deadline = Instant::now() + WAIT;
    let (_name, port, token) = loop {
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stdout={stdout:?} stderr={:?}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        std::thread::sleep(Duration::from_millis(5));
    };

    RunningSurface {
        port,
        token,
        stop,
        handle: Some(handle),
    }
}

struct HttpResp {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl HttpResp {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "invalid JSON (status {}): {e}; body={}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn send_raw(port: u16, raw: &[u8]) -> HttpResp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(raw).unwrap();
    stream.flush().unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap_or_else(|err| {
        panic!(
            "timed out or failed waiting for HTTP response on port {port}: {err}; partial={:?}",
            String::from_utf8_lossy(&buf)
        )
    });
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("missing HTTP header terminator");
    let head = std::str::from_utf8(&buf[..split]).unwrap();
    let body = buf[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let mut content_type = String::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-type") {
                content_type = v.trim().to_string();
            }
        }
    }
    HttpResp {
        status,
        content_type,
        body,
    }
}

fn post_attachment(surface: &RunningSurface, name: &str, bytes: &[u8]) -> HttpResp {
    let encoded: String = name
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect();
    let head = format!(
        "POST /api/session/attachments?name={encoded} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Blyte-Token: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        surface.port,
        surface.token,
        bytes.len()
    );
    let mut raw = head.into_bytes();
    raw.extend_from_slice(bytes);
    send_raw(surface.port, &raw)
}

fn get_attachment(surface: &RunningSurface, id: &str) -> HttpResp {
    let head = format!(
        "GET /api/session/attachments/{id} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Blyte-Token: {}\r\nConnection: close\r\n\r\n",
        surface.port, surface.token
    );
    send_raw(surface.port, head.as_bytes())
}

fn post_message(surface: &RunningSurface, payload: &serde_json::Value) -> HttpResp {
    let body = serde_json::to_vec(payload).unwrap();
    let head = format!(
        "POST /api/session/messages HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Blyte-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        surface.port,
        surface.token,
        body.len()
    );
    let mut raw = head.into_bytes();
    raw.extend_from_slice(&body);
    send_raw(surface.port, &raw)
}

fn get_events(surface: &RunningSurface) -> Vec<serde_json::Value> {
    let head = format!(
        "GET /api/session/events?after=0&wait=0 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Blyte-Token: {}\r\nConnection: close\r\n\r\n",
        surface.port, surface.token
    );
    let resp = send_raw(surface.port, head.as_bytes());
    assert_eq!(resp.status, 200, "{}", resp.text());
    resp.json()["entries"].as_array().unwrap().clone()
}

fn surface_record_dir(repo: &Path) -> PathBuf {
    let base = repo.join(".wt").join("surfaces");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(
        dirs.len(),
        1,
        "expected 1 surface dir in {}",
        base.display()
    );
    dirs.pop().unwrap()
}

fn wait_for_requests(
    surface: &RunningSurface,
    seen: &Arc<Mutex<Vec<Request>>>,
    count: usize,
) -> Vec<Request> {
    let deadline = Instant::now() + WAIT;
    loop {
        let snapshot = seen.lock().unwrap().clone();
        if snapshot.len() >= count {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {count} provider requests (have {}); events={:?}",
            snapshot.len(),
            get_events(surface)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_idle(surface: &RunningSurface) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + WAIT;
    loop {
        let entries = get_events(surface);
        if entries.iter().any(|e| e["kind"] == "idle") {
            return entries;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for idle entry; entries={entries:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn sample_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52,
    ]
}

#[test]
fn upload_then_message_sends_the_image_to_the_model() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, seen) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let png = sample_png();
    let upload = post_attachment(&surface, "shot.png", &png);
    assert_eq!(upload.status, 200, "{}", upload.text());
    let meta = upload.json();
    assert_eq!(meta["id"], "a1");
    assert_eq!(meta["name"], "shot.png");
    assert_eq!(meta["mime_type"], "image/png");
    assert_eq!(meta["size"], png.len());
    assert_eq!(meta["image"], true);

    let send = post_message(
        &surface,
        &serde_json::json!({
            "text": "look at this",
            "attachments": ["a1"]
        }),
    );
    assert_eq!(send.status, 200, "{}", send.text());

    let requests = wait_for_requests(&surface, &seen, 1);
    let last_msg = requests[0].messages.last().unwrap();
    let expected_saved = std::fs::canonicalize(
        surface_record_dir(repo.path())
            .join("attachments")
            .join("a1-shot.png"),
    )
    .unwrap();
    let expected_line = format!(
        "[Image #1] shot.png (image/png, {} bytes) saved at {}",
        png.len(),
        expected_saved.display()
    );
    match last_msg {
        Message::UserParts(parts) => {
            assert_eq!(parts.len(), 2, "parts={parts:?}");
            match &parts[0] {
                UserPart::Text(text) => {
                    assert!(
                        text.ends_with(&expected_line),
                        "expected text ending with {expected_line:?}, got {text:?}"
                    );
                }
                other => panic!("expected first part to be text, got {other:?}"),
            }
            match &parts[1] {
                UserPart::Image { mime_type, data } => {
                    assert_eq!(mime_type, "image/png");
                    assert_eq!(data.as_ref(), base64_encode(&png));
                }
                other => panic!("expected second part to be image, got {other:?}"),
            }
        }
        other => panic!("expected UserParts message, got {other:?}"),
    }

    let entries = wait_for_idle(&surface);
    let user_entry = entries
        .iter()
        .find(|e| e["kind"] == "user_message")
        .unwrap();
    assert_eq!(user_entry["attachments"][0]["id"], "a1");
    assert_eq!(user_entry["attachments"][0]["name"], "shot.png");
}

#[test]
fn a_non_image_file_is_saved_and_named_but_not_inlined() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, seen) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let body = b"line one\nline two\n";
    let upload = post_attachment(&surface, "notes.txt", body);
    assert_eq!(upload.status, 200, "{}", upload.text());
    let meta = upload.json();
    assert_eq!(meta["id"], "a1");
    assert_eq!(meta["mime_type"], "application/octet-stream");
    assert_eq!(meta["image"], false);

    let send = post_message(
        &surface,
        &serde_json::json!({
            "text": "check the notes",
            "attachments": ["a1"]
        }),
    );
    assert_eq!(send.status, 200, "{}", send.text());

    let requests = wait_for_requests(&surface, &seen, 1);
    let expected_saved = std::fs::canonicalize(
        surface_record_dir(repo.path())
            .join("attachments")
            .join("a1-notes.txt"),
    )
    .unwrap();
    assert_eq!(std::fs::read(&expected_saved).unwrap(), body);
    let expected_line = format!(
        "[File notes.txt] (application/octet-stream, {} bytes) saved at {}",
        body.len(),
        expected_saved.display()
    );
    match requests[0].messages.last().unwrap() {
        Message::User(text) => {
            assert!(
                text.ends_with(&expected_line),
                "expected text ending with {expected_line:?}, got {text:?}"
            );
        }
        other => panic!("expected non-image attachment to stay Message::User, got {other:?}"),
    }
}

#[test]
fn a_png_named_txt_is_detected_as_an_image_and_a_text_file_named_png_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let png = sample_png();
    assert_eq!(detect_image(&png), Some("image/png"));
    assert_eq!(detect_image(b"plain text, not a png"), None);

    let m1 = store(dir.path(), "disguised.txt", &png).unwrap();
    assert!(m1.image);
    assert_eq!(m1.mime_type, "image/png");

    let m2 = store(dir.path(), "fake.png", b"plain text, not a png").unwrap();
    assert!(!m2.image);
    assert_eq!(m2.mime_type, "application/octet-stream");
}

#[test]
fn an_upload_over_10_mib_is_refused_with_413_naming_the_file() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, _) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let head = format!(
        "POST /api/session/attachments?name=huge.png HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Blyte-Token: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        surface.port,
        surface.token,
        MAX_ATTACHMENT_BYTES + 1
    );
    let resp = send_raw(surface.port, head.as_bytes());
    assert_eq!(resp.status, 413, "{}", resp.text());
    assert_eq!(
        resp.text(),
        "huge.png is larger than the 10 MiB limit for one file"
    );
}

#[test]
fn an_empty_upload_is_refused() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, _) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let resp = post_attachment(&surface, "empty.png", b"");
    assert_eq!(resp.status, 400, "{}", resp.text());
    let err = resp.json()["error"].as_str().unwrap().to_string();
    assert!(err.contains("empty"), "got: {err}");
}

#[test]
fn a_message_with_an_unknown_attachment_id_is_refused_naming_it() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, _) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let resp = post_message(
        &surface,
        &serde_json::json!({
            "text": "hello",
            "attachments": ["a99"]
        }),
    );
    assert_eq!(resp.status, 400, "{}", resp.text());
    let err = resp.json()["error"].as_str().unwrap().to_string();
    assert!(
        err.contains("a99"),
        "expected error to name a99, got: {err}"
    );
}

#[test]
fn a_message_with_only_an_attachment_and_no_text_is_accepted() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, seen) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let png = sample_png();
    let upload = post_attachment(&surface, "only.png", &png);
    assert_eq!(upload.status, 200, "{}", upload.text());

    let resp = post_message(
        &surface,
        &serde_json::json!({
            "text": "",
            "attachments": ["a1"]
        }),
    );
    assert_eq!(resp.status, 200, "{}", resp.text());

    let requests = wait_for_requests(&surface, &seen, 1);
    match requests[0].messages.last().unwrap() {
        Message::UserParts(parts) => {
            assert_eq!(parts.len(), 2);
            match &parts[0] {
                UserPart::Text(text) => {
                    assert!(text.contains("[Image #1] only.png"), "got: {text:?}");
                }
                other => panic!("expected text part, got {other:?}"),
            }
        }
        other => panic!("expected UserParts, got {other:?}"),
    }
}

#[test]
fn a_paste_marker_is_expanded_for_the_model_and_kept_as_a_marker_in_the_log() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, seen) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let pasted_text = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa";
    let marker = "[Pasted text #1 +10 lines]";
    let send = post_message(
        &surface,
        &serde_json::json!({
            "text": format!("before {marker} after"),
            "pastes": [{ "n": 1, "text": pasted_text }]
        }),
    );
    assert_eq!(send.status, 200, "{}", send.text());

    let requests = wait_for_requests(&surface, &seen, 1);
    match requests[0].messages.last().unwrap() {
        Message::User(text) => {
            assert!(
                text.ends_with(&format!("before {pasted_text} after")),
                "unexpected prompt: {text:?}"
            );
            assert!(!text.contains(marker));
        }
        other => panic!("expected Message::User, got {other:?}"),
    }

    let entries = wait_for_idle(&surface);
    let user_entry = entries
        .iter()
        .find(|e| e["kind"] == "user_message")
        .unwrap();
    assert_eq!(user_entry["text"], format!("before {marker} after"));
    assert_eq!(user_entry["pastes"][0]["n"], 1);
    assert_eq!(user_entry["pastes"][0]["text"], pasted_text);

    let log_path = surface_record_dir(repo.path()).join("entries.jsonl");
    let log_raw = std::fs::read_to_string(&log_path).unwrap();
    let logged_user: Entry = log_raw
        .lines()
        .map(|l| serde_json::from_str::<Entry>(l).unwrap())
        .find(|e| matches!(e.body, EntryBody::UserMessage { .. }))
        .unwrap();
    match logged_user.body {
        EntryBody::UserMessage { text, pastes, .. } => {
            assert_eq!(text, format!("before {marker} after"));
            assert_eq!(
                pastes,
                vec![Paste {
                    n: 1,
                    text: pasted_text.to_string(),
                }]
            );
        }
        other => panic!("expected UserMessage, got {other:?}"),
    }
}

#[test]
fn a_paste_marker_without_its_text_is_refused() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, _) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let missing_entry = post_message(
        &surface,
        &serde_json::json!({
            "text": "look: [Pasted text #2 +14 lines]"
        }),
    );
    assert_eq!(missing_entry.status, 400, "{}", missing_entry.text());
    let err1 = missing_entry.json()["error"].as_str().unwrap().to_string();
    assert!(
        err1.contains("[Pasted text #2 +14 lines]"),
        "expected error to name marker, got: {err1}"
    );

    let unused_entry = post_message(
        &surface,
        &serde_json::json!({
            "text": "no marker here",
            "pastes": [{ "n": 3, "text": "unused" }]
        }),
    );
    assert_eq!(unused_entry.status, 400, "{}", unused_entry.text());
    let err2 = unused_entry.json()["error"].as_str().unwrap().to_string();
    assert!(
        err2.contains("[Pasted text #3]"),
        "expected error to name unused paste, got: {err2}"
    );
}

#[test]
fn an_old_user_message_entry_without_attachments_still_reads() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");
    let old_line = r#"{"seq":1,"ts":1700000001,"kind":"user_message","text":"from before","reports":[],"queued":false}"#;
    std::fs::write(&log_path, format!("{old_line}\n")).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedAgentRunner::new(dir.path().to_path_buf());
    let session =
        Session::start_with_log_io("recording".into(), Some(log_path), &mut err_buf, |_| {
            Box::new(runner)
        })
        .unwrap();

    let loaded = session.entries_after(0, Duration::ZERO);
    assert!(matches!(
        &loaded[0].body,
        EntryBody::UserMessage {
            text,
            reports,
            queued: false,
            attachments,
            pastes,
        } if text == "from before"
            && reports.is_empty()
            && attachments.is_empty()
            && pastes.is_empty()
    ));
}

#[test]
fn get_attachment_returns_the_bytes_and_type_and_404_for_an_unknown_id() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = StateDirGuard::set(xdg.path());
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let (runner, _) = ScriptedAgentRunner::new(repo.path().to_path_buf());
    let surface = start_surface(repo.path(), Box::new(runner));

    let png = sample_png();
    let upload = post_attachment(&surface, "img.png", &png);
    assert_eq!(upload.status, 200, "{}", upload.text());

    let fetched = get_attachment(&surface, "a1");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.content_type, "image/png");
    assert_eq!(fetched.body, png);

    let missing = get_attachment(&surface, "a99");
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(missing.json()["error"].as_str().unwrap().contains("a99"));

    let unauth_get = send_raw(
        surface.port,
        format!(
            "GET /api/session/attachments/a1 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            surface.port
        )
        .as_bytes(),
    );
    assert_eq!(unauth_get.status, 403);

    let unauth_post = send_raw(
        surface.port,
        format!(
            "POST /api/session/attachments?name=x.png HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
            surface.port
        )
        .as_bytes(),
    );
    assert_eq!(unauth_post.status, 403);
}

#[test]
fn the_file_name_is_sanitised() {
    let dir = tempfile::tempdir().unwrap();
    let png = sample_png();
    let meta = store(dir.path(), "../../x y.png", &png).unwrap();
    assert_eq!(meta.id, "a1");
    assert_eq!(meta.name, "x_y.png");

    let expected_path = attachments_dir(dir.path()).join("a1-x_y.png");
    assert!(
        expected_path.is_file(),
        "expected {}",
        expected_path.display()
    );
    let (loaded_meta, loaded_bytes) = load(dir.path(), "a1").unwrap();
    assert_eq!(loaded_meta, meta);
    assert_eq!(loaded_bytes, png);
}

fn start_surface_with_agent_runner(root: &Path) -> (RunningSurface, TaskConfig) {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let captured_task_cfg: Arc<Mutex<Option<TaskConfig>>> = Arc::new(Mutex::new(None));
    let root_bg = root.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let captured_bg = Arc::clone(&captured_task_cfg);
    let args = default_surface_args();
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(root_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_runner_factory(
            &args,
            cfg,
            &mut out,
            &mut err,
            &mut opener,
            stop_bg,
            Box::new(move |cfg, approver| {
                let runner = AgentRunner::try_new(cfg, approver)?;
                let task_cfg = runner.task_config_for_prompt("check attachment");
                *captured_bg.lock().unwrap() = Some(task_cfg);
                let tmpdir = runner.private_tmpdir().map(Path::to_path_buf);
                Ok((Box::new(runner) as Box<dyn PromptRunner>, tmpdir))
            }),
        )
    });

    let deadline = Instant::now() + WAIT;
    let (_name, port, token) = loop {
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stdout={stdout:?} stderr={:?}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        std::thread::sleep(Duration::from_millis(5));
    };

    let task_cfg = captured_task_cfg
        .lock()
        .unwrap()
        .take()
        .expect("AgentRunner factory did not run");
    (
        RunningSurface {
            port,
            token,
            stop,
            handle: Some(handle),
        },
        task_cfg,
    )
}

#[test]
fn a_saved_attachment_is_inside_the_read_roots() {
    for is_git in [true, false] {
        let xdg = tempfile::tempdir().unwrap();
        let _guard = StateDirGuard::set(xdg.path());
        let root = tempfile::tempdir().unwrap();
        if is_git {
            init_git_repo(root.path());
        }

        let (surface, task_cfg) = start_surface_with_agent_runner(root.path());

        let upload = post_attachment(&surface, "spec.txt", b"attachment content\n");
        assert_eq!(upload.status, 200, "{}", upload.text());

        let roots = blyte_agent::tools::read_roots();
        let saved = roots
            .iter()
            .map(|r| r.join("a1-spec.txt"))
            .find(|p| p.is_file());
        assert!(
            saved.is_some(),
            "expected saved attachment under read_roots (is_git={is_git}); roots={roots:?}"
        );
        let saved_path = saved.unwrap();
        let saved_dir = saved_path.parent().unwrap();
        assert!(
            task_cfg.allow_read.iter().any(|p| p == saved_dir),
            "expected AgentRunner allow_read to contain {} (is_git={is_git}); allow_read={:?}",
            saved_dir.display(),
            task_cfg.allow_read
        );

        let out = blyte_agent::tools::execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "read",
            &serde_json::json!({ "path": saved_path.to_string_lossy() }),
        );
        assert!(!out.is_error, "read tool failed: {}", out.content);
        assert!(
            out.content.contains("attachment content"),
            "unexpected read output: {}",
            out.content
        );

        match build_sandbox_profile(&task_cfg).unwrap() {
            sandbox::Profile::Seatbelt(sbpl) => {
                assert!(
                    sbpl.contains(&format!(
                        "(allow file-read-data (subpath \"{}\"))",
                        saved_dir.display()
                    )),
                    "expected sandbox profile to allow reading {} (is_git={is_git}): {sbpl}",
                    saved_dir.display()
                );
            }
            sandbox::Profile::LandlockConfined { read, .. } => {
                assert!(
                    read.iter().any(|p| p == saved_dir),
                    "expected Landlock read list to contain {} (is_git={is_git}): {read:?}",
                    saved_dir.display()
                );
            }
            sandbox::Profile::Landlock(_) | sandbox::Profile::LandlockOffline(_) => {}
        }

        if sandbox::available() {
            let shell = build_local_shell(&task_cfg).unwrap();
            let probe = shell
                .exec(
                    &format!("cat '{}'", saved_path.display()),
                    root.path(),
                    Duration::from_secs(30),
                )
                .unwrap();
            if !(probe.exit_code == 71 && probe.merged().contains("Operation not permitted")) {
                assert_eq!(
                    probe.exit_code,
                    0,
                    "sandboxed shell failed to read {} (is_git={is_git}): {}",
                    saved_path.display(),
                    probe.merged()
                );
                assert!(
                    probe.stdout.contains("attachment content"),
                    "unexpected sandboxed shell output (is_git={is_git}): {}",
                    probe.stdout
                );
            }
        }
    }
}
