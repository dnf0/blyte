use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_cli::clean::clean_surfaces;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::conversation_store::{conversation_path_for_id, ConversationStore};
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{run_with_hooks, TOKEN_HEADER};
use blyte_cli::session::{
    Entry, EntryBody, RESTART_NOTICE_TEXT, RESUME_INTERRUPTED_NOTICE_TEXT,
    RESUME_REMEMBERS_NOTICE_TEXT, RESUME_SUMMARIZED_NOTICE_TEXT,
};
use blyte_cli::surface_dir::{self, surface_dir, SurfaceRecord};
use blyte_provider::Message;

const WAIT: Duration = Duration::from_secs(30);

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvRestore {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvRestore {
    fn capture(keys: &[&'static str]) -> Self {
        let saved = keys.iter().map(|&k| (k, std::env::var(k).ok())).collect();
        Self { saved }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
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

fn init_git_repo(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    let cfg_path = root.join(".git/config");
    let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[user]\n\temail = test@example.com\n\tname = Test User\n");
    std::fs::write(&cfg_path, cfg).unwrap();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "init", "--no-gpg-sign"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
}

fn read_http_body(stream: &mut TcpStream) -> String {
    let mut buf = vec![0u8; 65536];
    let mut got = Vec::new();
    let mut header_end = None;
    let mut content_len = 0usize;
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        if header_end.is_none() {
            let text = String::from_utf8_lossy(&got);
            if let Some(i) = text.find("\r\n\r\n") {
                header_end = Some(i + 4);
                content_len = text[..i]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
            }
        }
        if let Some(end) = header_end {
            if got.len() >= end + content_len {
                break;
            }
        }
    }
    match header_end {
        Some(end) => String::from_utf8_lossy(&got[end..]).into_owned(),
        None => String::new(),
    }
}

struct FakeOpenAiServer {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeOpenAiServer {
    fn start(replies: Vec<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let reqs_bg = Arc::clone(&requests);
        let mut queue: VecDeque<String> = replies.into_iter().map(str::to_string).collect();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let req_body = read_http_body(&mut stream);
                reqs_bg.lock().unwrap().push(req_body);
                let reply = queue.pop_front().unwrap_or_else(|| "ok".to_string());
                let body = serde_json::json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": reply,
                        },
                        "finish_reason": "stop",
                    }],
                    "usage": {
                        "prompt_tokens": 20,
                        "completion_tokens": 10,
                        "total_tokens": 30,
                    }
                })
                .to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { port, requests }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn recorded_requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn default_repl_config(repo: PathBuf, model: &str) -> ReplConfig {
    ReplConfig {
        repo_root: repo,
        gate: None,
        context_strategy: "folded".to_string(),
        max_turns: 4,
        max_gates: 1,
        rollback: false,
        model: Some(model.to_string()),
        container: None,
        converse: true,
        fold_budget: Some(100_000),
        dispatch_cmd: None,
        sandbox: "off".to_string(),
        network: Some("on".to_string()),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
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

fn send_http(port: u16, req: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp
}

fn http_json_body(resp: &str) -> serde_json::Value {
    let (_, body) = resp.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

fn fetch_health(port: u16) -> serde_json::Value {
    let resp = send_http(
        port,
        &format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "health failed: {resp}");
    http_json_body(&resp)
}

fn post_message(port: u16, token: &str, text: &str) {
    let body = serde_json::json!({ "text": text }).to_string();
    let resp = send_http(
        port,
        &format!(
            "POST /api/session/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "POST /api/session/messages failed: {resp}"
    );
}

fn fetch_events(port: u16, token: &str) -> Vec<serde_json::Value> {
    let resp = send_http(
        port,
        &format!(
            "GET /api/session/events?after=0 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "GET /api/session/events failed: {resp}"
    );
    http_json_body(&resp)["entries"].as_array().unwrap().clone()
}

fn wait_for_idle_count(
    port: u16,
    token: &str,
    expected_idles: usize,
    out_buf: &Arc<Mutex<Vec<u8>>>,
    err_buf: &Arc<Mutex<Vec<u8>>>,
) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + WAIT;
    loop {
        let entries = fetch_events(port, token);
        let idle_count = entries.iter().filter(|e| e["kind"] == "idle").count();
        if idle_count >= expected_idles {
            return entries;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected_idles} idle entries; entries={entries:?}; stdout={:?}; stderr={:?}",
            String::from_utf8_lossy(&out_buf.lock().unwrap()),
            String::from_utf8_lossy(&err_buf.lock().unwrap()),
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct RunningSurface {
    name: String,
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    out_buf: Arc<Mutex<Vec<u8>>>,
    err_buf: Arc<Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<i32>,
}

impl RunningSurface {
    fn stop_and_join(self) -> i32 {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap()
    }
}

fn start_surface(repo: &Path, args: SurfaceArgs, model: &str) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let model_bg = model.to_string();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(repo_bg, &model_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_hooks(&args, cfg, &mut out, &mut err, &mut opener, stop_bg)
    });

    let deadline = Instant::now() + WAIT;
    let (name, port, token) = loop {
        let stdout = String::from_utf8_lossy(&out_buf.lock().unwrap()).into_owned();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for surface URL; stdout={stdout:?}; stderr={:?}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        std::thread::sleep(Duration::from_millis(5));
    };

    RunningSurface {
        name,
        port,
        token,
        stop,
        out_buf,
        err_buf,
        handle,
    }
}

fn setup_test_env(base_dir: &Path, openai_base_url: Option<&str>) -> EnvRestore {
    let restore = EnvRestore::capture(&[
        "XDG_STATE_HOME",
        "OPENAI_BASE_URL",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENROUTER_API_KEY",
        "OLLAMA_HOST",
        "OLLAMA_API_KEY",
        "BLYTE_MODEL",
        "BLYTE_USAGE_LOG",
        "BLYTE_NO_UPDATE_CHECK",
        "BLYTE_NO_KEEP_AWAKE",
    ]);
    std::env::set_var("XDG_STATE_HOME", base_dir.join("state"));
    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OPENROUTER_API_KEY");
    std::env::remove_var("OLLAMA_HOST");
    std::env::remove_var("OLLAMA_API_KEY");
    std::env::remove_var("BLYTE_MODEL");
    std::env::set_var("BLYTE_USAGE_LOG", "off");
    std::env::set_var("BLYTE_NO_UPDATE_CHECK", "1");
    std::env::set_var("BLYTE_NO_KEEP_AWAKE", "1");
    if let Some(url) = openai_base_url {
        std::env::set_var("OPENAI_BASE_URL", url);
        std::env::set_var("OPENAI_API_KEY", "test-key");
    } else {
        std::env::remove_var("OPENAI_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
    }
    restore
}

#[test]
fn resumed_surface_model_remembers_prior_conversation_and_shows_remembers_notice() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);

    let fake = FakeOpenAiServer::start(vec![
        "Understood, the code word is heron.",
        "The code word is heron.",
    ]);
    let _env = setup_test_env(tmp.path(), Some(&fake.base_url()));

    let first = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        "openai/gpt-test",
    );
    let surface_name = first.name.clone();
    post_message(first.port, &first.token, "the code word is heron");
    wait_for_idle_count(first.port, &first.token, 1, &first.out_buf, &first.err_buf);
    assert_eq!(first.stop_and_join(), 0);

    let resumed = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            resume: Some(surface_name),
            ..Default::default()
        },
        "openai/gpt-test",
    );
    post_message(resumed.port, &resumed.token, "what is the code word?");
    let entries = wait_for_idle_count(
        resumed.port,
        &resumed.token,
        2,
        &resumed.out_buf,
        &resumed.err_buf,
    );
    assert_eq!(resumed.stop_and_join(), 0);

    let requests = fake.recorded_requests();
    assert_eq!(requests.len(), 2, "expected 2 model requests: {requests:?}");
    let second_req = &requests[1];
    assert!(
        second_req.contains("the code word is heron"),
        "second request missing first user text: {second_req}"
    );
    assert!(
        second_req.contains("Understood, the code word is heron."),
        "second request missing first reply: {second_req}"
    );

    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "error" && e["message"] == RESUME_REMEMBERS_NOTICE_TEXT),
        "expected remembers notice in entries: {entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|e| e["kind"] == "error" && e["message"] == RESTART_NOTICE_TEXT),
        "unexpected old restart notice in entries: {entries:?}"
    );
}

#[test]
fn resume_with_corrupt_conversation_file_exits_1_and_leaves_record_unchanged() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);

    let fake = FakeOpenAiServer::start(vec!["first reply"]);
    let _env = setup_test_env(tmp.path(), Some(&fake.base_url()));

    let first = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        "openai/gpt-test",
    );
    let health = fetch_health(first.port);
    let surface_id = health["surface_id"].as_str().unwrap().to_string();
    let surface_name = first.name.clone();
    post_message(first.port, &first.token, "hello");
    wait_for_idle_count(first.port, &first.token, 1, &first.out_buf, &first.err_buf);
    assert_eq!(first.stop_and_join(), 0);

    let conv_path = conversation_path_for_id(&surface_id).unwrap();
    std::fs::write(&conv_path, "{\"version\":1}\nnot json\n").unwrap();

    let record_path = surface_dir(&repo, &surface_id).join("surface.json");
    let record_before = std::fs::read(&record_path).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let code = run_with_hooks(
        &SurfaceArgs {
            no_open: true,
            resume: Some(surface_name),
            ..Default::default()
        },
        default_repl_config(repo.clone(), "openai/gpt-test"),
        &mut out,
        &mut err,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    let stderr = String::from_utf8_lossy(&err);
    assert_eq!(code, 1, "expected exit code 1, stderr={stderr:?}");
    let expected_prefix = format!(
        "blyte: cannot read the saved conversation {} (line 2):",
        conv_path.display()
    );
    assert!(
        stderr.contains(&expected_prefix),
        "stderr missing expected prefix {expected_prefix:?}: {stderr:?}"
    );
    assert!(
        stderr.contains("delete that file to resume without the model's memory"),
        "stderr missing recovery hint: {stderr:?}"
    );
    assert_eq!(std::fs::read(&record_path).unwrap(), record_before);
}

#[test]
fn resume_without_conversation_file_shows_restart_notice_and_sends_no_prior_messages() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);

    let fake = FakeOpenAiServer::start(vec!["first reply", "second reply"]);
    let _env = setup_test_env(tmp.path(), Some(&fake.base_url()));

    let first = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        "openai/gpt-test",
    );
    let health = fetch_health(first.port);
    let surface_id = health["surface_id"].as_str().unwrap().to_string();
    let surface_name = first.name.clone();
    post_message(first.port, &first.token, "secret zebra phrase");
    wait_for_idle_count(first.port, &first.token, 1, &first.out_buf, &first.err_buf);
    assert_eq!(first.stop_and_join(), 0);

    let conv_path = conversation_path_for_id(&surface_id).unwrap();
    assert!(conv_path.exists());
    std::fs::remove_file(&conv_path).unwrap();

    let resumed = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            resume: Some(surface_name),
            ..Default::default()
        },
        "openai/gpt-test",
    );
    post_message(resumed.port, &resumed.token, "fresh question");
    let entries = wait_for_idle_count(
        resumed.port,
        &resumed.token,
        2,
        &resumed.out_buf,
        &resumed.err_buf,
    );
    assert_eq!(resumed.stop_and_join(), 0);

    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "error" && e["message"] == RESTART_NOTICE_TEXT),
        "expected RESTART_NOTICE_TEXT in entries: {entries:?}"
    );
    let requests = fake.recorded_requests();
    assert_eq!(requests.len(), 2);
    let second_req = &requests[1];
    assert!(
        !second_req.contains("secret zebra phrase"),
        "expected no earlier messages in resumed request: {second_req}"
    );
    assert!(
        !second_req.contains("first reply"),
        "expected no earlier reply in resumed request: {second_req}"
    );
}

#[test]
fn resume_with_entry_log_ending_in_user_message_shows_interrupted_notice() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);

    let fake = FakeOpenAiServer::start(vec!["first reply"]);
    let _env = setup_test_env(tmp.path(), Some(&fake.base_url()));

    let first = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        "openai/gpt-test",
    );
    let health = fetch_health(first.port);
    let surface_id = health["surface_id"].as_str().unwrap().to_string();
    let surface_name = first.name.clone();
    post_message(first.port, &first.token, "first message");
    wait_for_idle_count(first.port, &first.token, 1, &first.out_buf, &first.err_buf);
    assert_eq!(first.stop_and_join(), 0);

    let entries_path = surface_dir(&repo, &surface_id).join("entries.jsonl");
    let mut log_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&entries_path)
        .unwrap();
    let pending_entry = Entry {
        seq: 99,
        ts: 1_700_000_000,
        body: EntryBody::UserMessage {
            text: "interrupted prompt".to_string(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    writeln!(
        log_file,
        "{}",
        serde_json::to_string(&pending_entry).unwrap()
    )
    .unwrap();
    drop(log_file);

    let resumed = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            resume: Some(surface_name),
            ..Default::default()
        },
        "openai/gpt-test",
    );
    let entries = fetch_events(resumed.port, &resumed.token);
    assert_eq!(resumed.stop_and_join(), 0);

    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "error" && e["message"] == RESUME_INTERRUPTED_NOTICE_TEXT),
        "expected interrupted notice in entries: {entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|e| e["kind"] == "error" && e["message"] == RESUME_REMEMBERS_NOTICE_TEXT),
        "unexpected non-interrupted notice in entries: {entries:?}"
    );
}

#[test]
fn resume_on_different_provider_kind_digests_conversation_and_shows_summary_notice() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);

    let fake = FakeOpenAiServer::start(vec!["first reply", "second reply"]);
    let _env = setup_test_env(tmp.path(), Some(&fake.base_url()));

    let first = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        "openai/gpt-test",
    );
    let health = fetch_health(first.port);
    let surface_id = health["surface_id"].as_str().unwrap().to_string();
    let surface_name = first.name.clone();
    post_message(first.port, &first.token, "remember osprey");
    wait_for_idle_count(first.port, &first.token, 1, &first.out_buf, &first.err_buf);
    assert_eq!(first.stop_and_join(), 0);

    let conv_path = conversation_path_for_id(&surface_id).unwrap();
    let (mut store, _) = ConversationStore::load(conv_path.clone()).unwrap();
    store
        .rewrite(
            Some("anthropic/claude-sonnet-4-5"),
            Some("remember osprey"),
            &[
                Message::User("remember osprey".to_string()),
                Message::Assistant("I will remember osprey".to_string()),
            ],
        )
        .unwrap();

    let resumed = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            resume: Some(surface_name),
            ..Default::default()
        },
        "openai/gpt-test",
    );
    post_message(resumed.port, &resumed.token, "what bird?");
    let entries = wait_for_idle_count(
        resumed.port,
        &resumed.token,
        2,
        &resumed.out_buf,
        &resumed.err_buf,
    );
    assert_eq!(resumed.stop_and_join(), 0);

    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "error" && e["message"] == RESUME_SUMMARIZED_NOTICE_TEXT),
        "expected summarized notice in entries: {entries:?}"
    );
    let requests = fake.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].contains("Earlier turns, folded"),
        "expected digest in second request: {}",
        requests[1]
    );
    assert!(
        requests[1].contains("osprey"),
        "expected osprey in digest: {}",
        requests[1]
    );
}

#[test]
fn clean_removes_conversation_file_for_old_stopped_surface() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);
    let _env = setup_test_env(tmp.path(), None);

    let now_secs = 1_800_000_000u64;
    let day_secs = 86_400u64;
    let id = "s0123456789abcdef";
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let unused_port = listener.local_addr().unwrap().port();
    drop(listener);

    let record = SurfaceRecord {
        id: id.to_string(),
        name: "main".to_string(),
        pid: 99999,
        port: unused_port,
        session_id: "sess-old".to_string(),
        started_at: now_secs - 20 * day_secs,
    };
    let sdir = surface_dir(&repo, id);
    surface_dir::write_record_in(&sdir, &record).unwrap();
    let token_path = blyte_agent::sandbox::surface_token_path_for_id(id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, "tok\n").unwrap();
    let conv_path = conversation_path_for_id(id).unwrap();
    let mut store = ConversationStore::new_for_fresh(conv_path.clone()).unwrap();
    store
        .save("openai/gpt-test", None, &[Message::User("old".to_string())])
        .unwrap();
    assert!(conv_path.exists());

    let old_ts = std::time::UNIX_EPOCH + Duration::from_secs(now_secs - 20 * day_secs);
    std::fs::OpenOptions::new()
        .write(true)
        .open(sdir.join("surface.json"))
        .unwrap()
        .set_modified(old_ts)
        .unwrap();

    let lines = clean_surfaces(&repo, 14, false, now_secs).unwrap();
    assert_eq!(
        lines.len(),
        3,
        "expected dir, token, conversation removed: {lines:?}"
    );
    assert!(!sdir.exists());
    assert!(!token_path.exists());
    assert!(!conv_path.exists());
}
