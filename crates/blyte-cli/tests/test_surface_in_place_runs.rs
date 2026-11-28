use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_agent::sandbox::surface_token_path_for_id;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::dispatch_in_place::records_dir_for;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::run_with_hooks;
use blyte_cli::surface_dir::{folder_store, write_record_in, SurfaceRecord};
use blyte_cli::surface_notify::{notify_runs_changed, NotifyOutcome};
use blyte_cli::surface_registry::state_dir;

const WAIT: Duration = Duration::from_secs(30);

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

fn ensure_isolated_state_home() {
    static ISOLATED_STATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    let _ = ISOLATED_STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_STATE_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("BLYTE_STATE_DIR", dir.path().join("blyte"));
        dir
    });
}

fn default_repl_config(repo_root: PathBuf) -> ReplConfig {
    ensure_isolated_state_home();
    ReplConfig {
        repo_root,
        gate: None,
        context_strategy: "folded".to_string(),
        max_turns: 25,
        max_gates: 5,
        rollback: true,
        model: Some("gemini-4-neon".to_string()),
        container: None,
        converse: false,
        fold_budget: None,
        dispatch_cmd: None,
        sandbox: "on".to_string(),
        network: None,
        allow_read: Vec::new(),
        pass_env: Vec::new(),
    }
}

fn default_surface_args() -> SurfaceArgs {
    SurfaceArgs {
        no_open: true,
        ..SurfaceArgs::default()
    }
}

fn parse_surface_ready_line(line: &str) -> Option<(String, u16, String)> {
    let rest = line.strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at http://127.0.0.1:")?;
    let (port_str, token) = url.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    Some((name.to_string(), port, token.to_string()))
}

static SURFACE_POLICY_LOCK: RwLock<()> = RwLock::new(());

struct RunningSurface {
    port: u16,
    token: String,
    surface_id: String,
    stop: Arc<AtomicBool>,
    err_buf: Arc<Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<i32>,
    _policy_guard: std::sync::RwLockReadGuard<'static, ()>,
}

impl RunningSurface {
    fn stop_and_join(self) -> (i32, String) {
        self.stop.store(true, Ordering::Relaxed);
        let code = self.handle.join().unwrap();
        let stderr = String::from_utf8_lossy(&self.err_buf.lock().unwrap()).into_owned();
        drop(self._policy_guard);
        (code, stderr)
    }
}

fn start_surface(dir: &Path) -> RunningSurface {
    let policy_guard = SURFACE_POLICY_LOCK
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let dir_bg = dir.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let args = default_surface_args();
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(dir_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_hooks(&args, cfg, &mut out, &mut err, &mut opener, stop_bg)
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

    let (status, body) = http_request(port, "GET", "/api/health", None, None);
    assert_eq!(status, 200, "body={body}");
    let health: serde_json::Value = serde_json::from_str(&body).unwrap();
    let surface_id = health["surface_id"].as_str().unwrap().to_string();

    RunningSurface {
        port,
        token,
        surface_id,
        stop,
        err_buf,
        handle,
        _policy_guard: policy_guard,
    }
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("X-Blyte-Token: {tok}\r\n"));
    }
    if let Some(payload) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    if let Some(payload) = body {
        req.push_str(payload);
    }
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap_or_else(|err| {
        panic!(
            "timed out or failed waiting for HTTP response to {method} {path}: {err}; partial={:?}",
            String::from_utf8_lossy(&raw)
        )
    });
    let text = String::from_utf8(raw).unwrap();
    let (head, resp_body) = text.split_once("\r\n\r\n").unwrap();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, resp_body.to_string())
}

fn git_init_with_commit(dir: &Path) {
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "init"]);
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write_finished_in_place_run(
    store: &Path,
    id: &str,
    surface_id: &str,
    rel_file: &str,
    before_text: &str,
    after_text: &str,
    diff_text: &str,
) {
    std::fs::create_dir_all(store).unwrap();
    let started_at = now_secs().saturating_sub(60);
    std::fs::write(
        store.join(format!("{id}.task.md")),
        format!("Edit {rel_file}\n"),
    )
    .unwrap();
    let start = serde_json::json!({
        "id": id,
        "started_at": started_at,
        "pid": 999999u32,
        "in_place": true,
        "research": false,
        "surface": surface_id,
        "request": "req-1",
    });
    std::fs::write(
        store.join(format!("{id}.start.json")),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();

    let summary = serde_json::json!({
        "id": id,
        "outcome": "finished",
        "turns": 2,
        "cost_usd": 0.01,
        "seconds": 30,
        "final_message": format!("Updated {rel_file}"),
        "changed_files": [rel_file],
        "commits": [],
    });
    std::fs::write(
        store.join(format!("{id}.json")),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();

    let change_line = serde_json::json!({
        "path": rel_file,
        "existed": true,
    })
    .to_string();
    std::fs::write(
        store.join(format!("{id}.changes.json")),
        format!("{change_line}\n"),
    )
    .unwrap();

    let before_path = store.join(format!("{id}.before")).join(rel_file);
    std::fs::create_dir_all(before_path.parent().unwrap()).unwrap();
    std::fs::write(&before_path, before_text).unwrap();

    let after_path = store.join(format!("{id}.after")).join(rel_file);
    std::fs::create_dir_all(after_path.parent().unwrap()).unwrap();
    std::fs::write(&after_path, after_text).unwrap();

    std::fs::write(store.join(format!("{id}.diff")), diff_text).unwrap();
    std::fs::write(store.join(format!("{id}.log")), "").unwrap();
}

#[test]
fn api_runs_lists_finished_and_running_in_place_runs_and_reports_to_conversation() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notes-list");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "new line\n").unwrap();

    let running = start_surface(&folder);
    let store = records_dir_for(&folder).unwrap();

    write_finished_in_place_run(
        &store,
        "T1",
        &running.surface_id,
        "notes.txt",
        "old line\n",
        "new line\n",
        "--- a/notes.txt\n+++ b/notes.txt\n@@ -1 +1 @@\n-old line\n+new line\n",
    );

    std::fs::write(store.join("T2.task.md"), "Still going\n").unwrap();
    let running_start = serde_json::json!({
        "id": "T2",
        "started_at": now_secs().saturating_sub(10),
        "pid": std::process::id(),
        "in_place": true,
        "research": false,
        "surface": running.surface_id,
    });
    std::fs::write(
        store.join("T2.start.json"),
        serde_json::to_string_pretty(&running_start).unwrap(),
    )
    .unwrap();

    write_finished_in_place_run(
        &store,
        "T_OTHER",
        "s0000000000000999",
        "notes.txt",
        "a\n",
        "b\n",
        "",
    );

    let (status, body) = http_request(running.port, "GET", "/api/runs", Some(&running.token), None);
    assert_eq!(status, 200, "body={body}");
    let val: serde_json::Value = serde_json::from_str(&body).unwrap();

    let live = val["running"].as_array().unwrap();
    assert_eq!(live.len(), 1, "running={live:?}");
    assert_eq!(live[0]["id"], "T2");
    assert_eq!(live[0]["title"], "Still going");

    let runs = val["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "runs={runs:?}");
    assert_eq!(runs[0]["id"], "T1");
    assert_eq!(runs[0]["changed_files"], serde_json::json!(["notes.txt"]));
    assert_eq!(runs[0]["commits"], serde_json::json!([]));
    assert_eq!(runs[0]["in_place"], serde_json::json!(true));
    assert!(runs[0].get("undone").is_none());
    assert_eq!(runs[0]["card"], "clean");

    let (ev_status, ev_body) = http_request(
        running.port,
        "GET",
        "/api/session/events?after=0",
        Some(&running.token),
        None,
    );
    assert_eq!(ev_status, 200, "body={ev_body}");
    let ev_val: serde_json::Value = serde_json::from_str(&ev_body).unwrap();
    let entries = ev_val["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| { e["kind"] == "decision" && e["run"] == "T1" && e["action"] == "finished" }),
        "expected finished in-place run T1 to be reported to conversation; entries={entries:?}"
    );
    assert!(store.join("T1.reported.json").exists());

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn api_runs_diff_returns_saved_diff_and_report_and_errors_for_unknown_id() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notes-diff");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "new line\n").unwrap();

    let running = start_surface(&folder);
    let store = records_dir_for(&folder).unwrap();

    write_finished_in_place_run(
        &store,
        "T1",
        &running.surface_id,
        "notes.txt",
        "old line\n",
        "new line\n",
        "--- a/notes.txt\n+++ b/notes.txt\n@@ -1 +1 @@\n-old line\n+new line\n",
    );

    std::fs::write(store.join("R1.task.md"), "Read notes\n").unwrap();
    let research_start = serde_json::json!({
        "id": "R1",
        "started_at": now_secs().saturating_sub(60),
        "pid": 999999u32,
        "in_place": true,
        "research": true,
        "surface": running.surface_id,
    });
    std::fs::write(
        store.join("R1.start.json"),
        serde_json::to_string_pretty(&research_start).unwrap(),
    )
    .unwrap();
    let research_summary = serde_json::json!({
        "id": "R1",
        "outcome": "finished",
        "turns": 1,
        "cost_usd": 0.01,
        "seconds": 15,
        "final_message": "done",
    });
    std::fs::write(
        store.join("R1.json"),
        serde_json::to_string_pretty(&research_summary).unwrap(),
    )
    .unwrap();
    std::fs::write(store.join("R1.report.md"), "# Findings\nAll clear.\n").unwrap();
    std::fs::write(store.join("R1.log"), "").unwrap();

    let (status, body) = http_request(
        running.port,
        "GET",
        "/api/runs/T1/diff",
        Some(&running.token),
        None,
    );
    assert_eq!(status, 200, "body={body}");
    assert!(body.contains("== files"), "body={body}");
    assert!(body.contains("modified notes.txt"), "body={body}");
    assert!(body.contains("-old line"), "body={body}");
    assert!(body.contains("+new line"), "body={body}");

    let (r_status, r_body) = http_request(
        running.port,
        "GET",
        "/api/runs/R1/diff",
        Some(&running.token),
        None,
    );
    assert_eq!(r_status, 200, "body={r_body}");
    assert!(r_body.contains("# Findings"), "body={r_body}");
    assert!(r_body.contains("All clear."), "body={r_body}");

    let (missing_status, missing_body) = http_request(
        running.port,
        "GET",
        "/api/runs/UNKNOWN/diff",
        Some(&running.token),
        None,
    );
    assert_eq!(missing_status, 404, "body={missing_body}");
    assert!(missing_body.contains("UNKNOWN"), "body={missing_body}");

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn decision_route_records_verdict_and_git_actions_stay_409() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notes-decision");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "new line\n").unwrap();

    let running = start_surface(&folder);
    let store = records_dir_for(&folder).unwrap();

    write_finished_in_place_run(
        &store,
        "T1",
        &running.surface_id,
        "notes.txt",
        "old line\n",
        "new line\n",
        "--- a/notes.txt\n+++ b/notes.txt\n@@ -1 +1 @@\n-old line\n+new line\n",
    );

    let (status, body) = http_request(
        running.port,
        "POST",
        "/api/runs/T1/decision",
        Some(&running.token),
        Some(r#"{"action":"accept","note":"looks good"}"#),
    );
    assert_eq!(status, 200, "body={body}");

    let verdict_path = store.join("T1.verdict.json");
    assert!(
        verdict_path.is_file(),
        "expected verdict file at {}",
        verdict_path.display()
    );
    let verdict: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(verdict["verdict"], "accepted");
    assert_eq!(verdict["notes"], "looks good");

    let expected_git_err = serde_json::json!({ "error": "this folder is not a git repository" });
    let (push_status, push_body) = http_request(
        running.port,
        "POST",
        "/api/requests/req-1/push",
        Some(&running.token),
        Some("{}"),
    );
    assert_eq!(push_status, 409, "body={push_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&push_body).unwrap(),
        expected_git_err
    );

    let (pr_status, pr_body) = http_request(
        running.port,
        "POST",
        "/api/requests/req-1/pull-request",
        Some(&running.token),
        Some(r#"{"title":"t","body":"b"}"#),
    );
    assert_eq!(pr_status, 409, "body={pr_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&pr_body).unwrap(),
        expected_git_err
    );

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn undo_route_restores_file_rejects_second_undo_and_requires_token() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notes-undo");
    std::fs::create_dir_all(&folder).unwrap();
    let target = folder.join("notes.txt");
    std::fs::write(&target, "new line\n").unwrap();

    let running = start_surface(&folder);
    let store = records_dir_for(&folder).unwrap();

    write_finished_in_place_run(
        &store,
        "T1",
        &running.surface_id,
        "notes.txt",
        "old line\n",
        "new line\n",
        "--- a/notes.txt\n+++ b/notes.txt\n@@ -1 +1 @@\n-old line\n+new line\n",
    );

    let (no_tok_status, no_tok_body) =
        http_request(running.port, "POST", "/api/runs/T1/undo", None, Some("{}"));
    assert_eq!(no_tok_status, 403, "body={no_tok_body}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new line\n");

    let (status, body) = http_request(
        running.port,
        "POST",
        "/api/runs/T1/undo",
        Some(&running.token),
        Some("{}"),
    );
    assert_eq!(status, 200, "body={body}");
    let val: serde_json::Value = serde_json::from_str(&body).unwrap();
    let lines = val["lines"].as_array().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.as_str() == Some("restored notes.txt")),
        "lines={lines:?}"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old line\n");

    let (runs_status, runs_body) =
        http_request(running.port, "GET", "/api/runs", Some(&running.token), None);
    assert_eq!(runs_status, 200, "body={runs_body}");
    let runs_val: serde_json::Value = serde_json::from_str(&runs_body).unwrap();
    let t1 = &runs_val["runs"][0];
    assert_eq!(t1["undone"], serde_json::json!(true));
    assert!(
        t1["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap_or("").contains("was undone")),
        "flags={:?}",
        t1["flags"]
    );

    let (second_status, second_body) = http_request(
        running.port,
        "POST",
        "/api/runs/T1/undo",
        Some(&running.token),
        Some("{}"),
    );
    assert_eq!(second_status, 409, "body={second_body}");
    assert!(
        second_body.contains("already undone"),
        "expected already undone refusal, got: {second_body}"
    );

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn undo_for_run_in_git_repository_answers_409() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("git-repo-undo");
    std::fs::create_dir_all(&repo).unwrap();
    git_init_with_commit(&repo);

    let running = start_surface(&repo);

    let wt_dir = repo.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    let start = serde_json::json!({
        "id": "T1",
        "title": "Repo run",
        "task": "repo task",
        "started_at": 1700000000u64,
        "pid": 999999u32,
        "research": false,
        "surface": running.surface_id,
    });
    std::fs::write(
        wt_dir.join("T1.start.json"),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();

    let (status, body) = http_request(
        running.port,
        "POST",
        "/api/runs/T1/undo",
        Some(&running.token),
        Some("{}"),
    );
    assert_eq!(status, 409, "body={body}");
    assert!(
        body.contains("undo is for runs in a folder that is not a git repository"),
        "body={body}"
    );

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn surface_notify_reaches_non_repository_surface_record() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notify-plain");
    std::fs::create_dir_all(&folder).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Arc<Mutex<Option<(String, String, String)>>> = Arc::new(Mutex::new(None));
    let received_bg = Arc::clone(&received);

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap_or_else(|err| {
            panic!("failed reading notify request line: {err}; saw={request_line:?}")
        });
        let mut token = String::new();
        let mut content_len = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap_or_else(|err| {
                panic!(
                    "failed reading notify header line: {err}; request_line={request_line:?} saw={line:?}"
                )
            });
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                if k.eq_ignore_ascii_case("x-blyte-token") {
                    token = v.trim().to_string();
                } else if k.eq_ignore_ascii_case("content-length") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0u8; content_len];
        reader.read_exact(&mut body).unwrap_or_else(|err| {
            panic!(
                "failed reading {content_len}-byte notify body: {err}; request_line={request_line:?}"
            )
        });
        let body_str = String::from_utf8(body).unwrap();
        *received_bg.lock().unwrap() = Some((request_line.trim().to_string(), token, body_str));
        let resp = "HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        stream.write_all(resp.as_bytes()).unwrap();
    });

    let surface_id = "s0000000000000777";
    let token = "plain-folder-notify-token";
    let sd = state_dir().unwrap();
    let fstore = folder_store(&sd, &folder).unwrap();
    let sdir = fstore.join("surfaces").join(surface_id);
    std::fs::create_dir_all(&sdir).unwrap();
    let record = SurfaceRecord {
        id: surface_id.to_string(),
        name: "notify-plain".to_string(),
        pid: std::process::id(),
        port,
        session_id: format!("session-{surface_id}"),
        started_at: 1700000000,
    };
    write_record_in(&sdir, &record).unwrap();
    let token_path = surface_token_path_for_id(surface_id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();

    let run_store = records_dir_for(&folder).unwrap();
    std::fs::create_dir_all(&run_store).unwrap();
    let start = serde_json::json!({
        "id": "T1",
        "title": "Notify test",
        "task": "notify",
        "started_at": 1700000000u64,
        "pid": 999999u32,
        "research": false,
        "surface": surface_id,
    });
    std::fs::write(
        run_store.join("T1.start.json"),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();

    let outcome = notify_runs_changed(&folder, "T1");
    assert_eq!(outcome, NotifyOutcome::Told);

    server.join().unwrap();
    let got = received
        .lock()
        .unwrap()
        .clone()
        .expect("listener must receive POST /api/runs/changed");
    assert_eq!(got.0, "POST /api/runs/changed HTTP/1.1");
    assert_eq!(got.1, token);
}

#[test]
fn surface_notify_uses_folder_store_for_non_repo_even_when_wt_dir_exists() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notify-plain-with-wt");
    std::fs::create_dir_all(folder.join(".wt")).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    let received_bg = Arc::clone(&received);

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap_or_else(|err| {
            panic!("failed reading notify request line: {err}; saw={request_line:?}")
        });
        let mut token = String::new();
        let mut content_len = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap_or_else(|err| {
                panic!(
                    "failed reading notify header line: {err}; request_line={request_line:?} saw={line:?}"
                )
            });
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                if k.eq_ignore_ascii_case("x-blyte-token") {
                    token = v.trim().to_string();
                } else if k.eq_ignore_ascii_case("content-length") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0u8; content_len];
        reader.read_exact(&mut body).unwrap_or_else(|err| {
            panic!(
                "failed reading {content_len}-byte notify body: {err}; request_line={request_line:?}"
            )
        });
        *received_bg.lock().unwrap() = Some((request_line.trim().to_string(), token));
        let resp = "HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        stream.write_all(resp.as_bytes()).unwrap();
    });

    let surface_id = "s0000000000000888";
    let token = "plain-folder-with-wt-token";
    let sd = state_dir().unwrap();
    let fstore = folder_store(&sd, &folder).unwrap();
    let sdir = fstore.join("surfaces").join(surface_id);
    std::fs::create_dir_all(&sdir).unwrap();
    let record = SurfaceRecord {
        id: surface_id.to_string(),
        name: "notify-plain-with-wt".to_string(),
        pid: std::process::id(),
        port,
        session_id: format!("session-{surface_id}"),
        started_at: 1700000000,
    };
    write_record_in(&sdir, &record).unwrap();
    let token_path = surface_token_path_for_id(surface_id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();

    let run_store = records_dir_for(&folder).unwrap();
    std::fs::create_dir_all(&run_store).unwrap();
    let start = serde_json::json!({
        "id": "T1",
        "started_at": 1700000000u64,
        "pid": 999999u32,
        "research": false,
        "surface": surface_id,
    });
    std::fs::write(
        run_store.join("T1.start.json"),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();

    let outcome = notify_runs_changed(&folder, "T1");
    assert_eq!(outcome, NotifyOutcome::Told);

    server.join().unwrap();
    let got = received
        .lock()
        .unwrap()
        .clone()
        .expect("listener must receive POST /api/runs/changed");
    assert_eq!(got.0, "POST /api/runs/changed HTTP/1.1");
    assert_eq!(got.1, token);
}

#[test]
fn in_place_review_diff_and_decide_reject_bad_start_files() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    std::fs::create_dir_all(&store).unwrap();

    write_finished_in_place_run(&store, "T_BAD_RES", "s1", "notes.txt", "old\n", "new\n", "");
    std::fs::write(
        store.join("T_BAD_RES.start.json"),
        serde_json::json!({
            "id": "T_BAD_RES",
            "started_at": now_secs().saturating_sub(60),
            "pid": 999999u32,
            "research": "yes",
        })
        .to_string(),
    )
    .unwrap();

    let diff_err = blyte_cli::in_place_review::diff_for_run(&store, "T_BAD_RES").unwrap_err();
    assert!(
        diff_err.contains("research") && diff_err.contains("T_BAD_RES.start.json"),
        "diff_err={diff_err}"
    );

    let decide_bad_res = blyte_cli::queue_actions::decide_in_place(
        &store,
        "T_BAD_RES",
        blyte_cli::queue_actions::Action::Accept,
        None,
        now_secs(),
    )
    .unwrap_err();
    assert!(
        decide_bad_res.contains("research") && decide_bad_res.contains("T_BAD_RES.start.json"),
        "decide_bad_res={decide_bad_res}"
    );

    write_finished_in_place_run(
        &store,
        "T_CORRUPT_START",
        "s1",
        "notes.txt",
        "old\n",
        "new\n",
        "",
    );
    std::fs::write(store.join("T_CORRUPT_START.start.json"), "{not json").unwrap();
    let decide_corrupt = blyte_cli::queue_actions::decide_in_place(
        &store,
        "T_CORRUPT_START",
        blyte_cli::queue_actions::Action::Accept,
        None,
        now_secs(),
    )
    .unwrap_err();
    assert!(
        decide_corrupt.contains("T_CORRUPT_START.start.json"),
        "decide_corrupt={decide_corrupt}"
    );

    write_finished_in_place_run(
        &store,
        "T_MISSING_START",
        "s1",
        "notes.txt",
        "old\n",
        "new\n",
        "",
    );
    std::fs::remove_file(store.join("T_MISSING_START.start.json")).unwrap();
    let decide_missing = blyte_cli::queue_actions::decide_in_place(
        &store,
        "T_MISSING_START",
        blyte_cli::queue_actions::Action::Accept,
        None,
        now_secs(),
    )
    .unwrap_err();
    assert!(
        decide_missing.contains("T_MISSING_START.start.json"),
        "decide_missing={decide_missing}"
    );
}

#[test]
fn collect_in_place_records_skipped_errors_without_fallbacks() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("notes-strict");
    let store = tmp.path().join("store");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    let now = now_secs();

    write_finished_in_place_run(&store, "T_BAD_RES", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::write(
        store.join("T_BAD_RES.start.json"),
        serde_json::json!({
            "id": "T_BAD_RES",
            "started_at": now.saturating_sub(60),
            "pid": 999999u32,
            "research": 42,
        })
        .to_string(),
    )
    .unwrap();

    write_finished_in_place_run(&store, "T_BAD_TASKFILE", "s1", "a.txt", "a\n", "b\n", "");
    let missing_task = tmp.path().join("does-not-exist-task.md");
    std::fs::write(
        store.join("T_BAD_TASKFILE.start.json"),
        serde_json::json!({
            "id": "T_BAD_TASKFILE",
            "started_at": now.saturating_sub(60),
            "pid": 999999u32,
            "task_file": missing_task.display().to_string(),
        })
        .to_string(),
    )
    .unwrap();

    write_finished_in_place_run(&store, "T_NO_TASKMD", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::remove_file(store.join("T_NO_TASKMD.task.md")).unwrap();

    write_finished_in_place_run(&store, "T_NO_SECONDS", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::write(
        store.join("T_NO_SECONDS.json"),
        serde_json::json!({
            "id": "T_NO_SECONDS",
            "outcome": "finished",
            "turns": 2,
            "changed_files": ["a.txt"],
        })
        .to_string(),
    )
    .unwrap();

    write_finished_in_place_run(&store, "T_NO_TURNS", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::write(
        store.join("T_NO_TURNS.json"),
        serde_json::json!({
            "id": "T_NO_TURNS",
            "outcome": "finished",
            "seconds": 10,
            "changed_files": ["a.txt"],
        })
        .to_string(),
    )
    .unwrap();

    write_finished_in_place_run(&store, "T_BAD_TURNS", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::write(
        store.join("T_BAD_TURNS.json"),
        serde_json::json!({
            "id": "T_BAD_TURNS",
            "outcome": "finished",
            "seconds": 10,
            "turns": "three",
            "changed_files": ["a.txt"],
        })
        .to_string(),
    )
    .unwrap();

    write_finished_in_place_run(&store, "T_BAD_VERDICT", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::write(store.join("T_BAD_VERDICT.verdict.json"), "{broken").unwrap();

    write_finished_in_place_run(&store, "T_UNREAD_VERDICT", "s1", "a.txt", "a\n", "b\n", "");
    std::fs::create_dir_all(store.join("T_UNREAD_VERDICT.verdict.json")).unwrap();

    std::fs::write(store.join("T_BAD_LOG.task.md"), "Running task\n").unwrap();
    std::fs::write(
        store.join("T_BAD_LOG.start.json"),
        serde_json::json!({
            "id": "T_BAD_LOG",
            "started_at": now.saturating_sub(10),
            "pid": std::process::id(),
        })
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(store.join("T_BAD_LOG.log")).unwrap();

    let live = blyte_cli::live_runs::collect_in_place(&folder, &store, now, 14).unwrap();
    assert!(live.runs.is_empty(), "runs={:?}", live.runs);
    assert!(live.running.is_empty(), "running={:?}", live.running);

    let reason_for = |id: &str| -> String {
        live.skipped
            .iter()
            .find(|s| s.key.ends_with(&format!("/{id}")))
            .unwrap_or_else(|| panic!("expected {id} in skipped: {:?}", live.skipped))
            .reason
            .clone()
    };

    assert!(reason_for("T_BAD_RES").contains("research"));
    assert!(reason_for("T_BAD_TASKFILE").contains("does-not-exist-task.md"));
    assert!(reason_for("T_NO_TASKMD").contains("T_NO_TASKMD.task.md"));
    assert!(reason_for("T_NO_SECONDS").contains("seconds"));
    assert!(reason_for("T_NO_TURNS").contains("turns"));
    assert!(reason_for("T_BAD_TURNS").contains("turns"));
    assert!(reason_for("T_BAD_VERDICT").contains("T_BAD_VERDICT.verdict.json"));
    assert!(reason_for("T_UNREAD_VERDICT").contains("T_UNREAD_VERDICT.verdict.json"));
    assert!(reason_for("T_BAD_LOG").contains("T_BAD_LOG.log"));
}
