use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyte_agent::approval::Approver;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::run_with_runner_factory;
use blyte_cli::session::{EntryBody, PromptRunner};
use blyte_cli::surface_registry::{live_entries, registry_dir, RegistryEntry, SurfaceState};

const WAIT_DEADLINE: Duration = Duration::from_secs(30);

static STATE_DIR_LOCK: Mutex<()> = Mutex::new(());

struct IsolatedStateDir {
    _dir: tempfile::TempDir,
    reg_dir: PathBuf,
    prev_xdg_state: Option<std::ffi::OsString>,
    prev_xdg_config: Option<std::ffi::OsString>,
    prev_blyte_state: Option<std::ffi::OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl IsolatedStateDir {
    fn new() -> Self {
        let lock = STATE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev_xdg_state = std::env::var_os("XDG_STATE_HOME");
        let prev_xdg_config = std::env::var_os("XDG_CONFIG_HOME");
        let prev_blyte_state = std::env::var_os("BLYTE_STATE_DIR");
        let blyte_state = dir.path().join("blyte");
        std::env::set_var("XDG_STATE_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("BLYTE_STATE_DIR", &blyte_state);
        let reg_dir = registry_dir().unwrap();
        Self {
            _dir: dir,
            reg_dir,
            prev_xdg_state,
            prev_xdg_config,
            prev_blyte_state,
            _lock: lock,
        }
    }

    fn registry_dir(&self) -> &Path {
        &self.reg_dir
    }
}

impl Drop for IsolatedStateDir {
    fn drop(&mut self) {
        match &self.prev_xdg_state {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        match &self.prev_xdg_config {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &self.prev_blyte_state {
            Some(v) => std::env::set_var("BLYTE_STATE_DIR", v),
            None => std::env::remove_var("BLYTE_STATE_DIR"),
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

#[derive(Clone)]
struct Gate {
    pair: Arc<(Mutex<bool>, Condvar)>,
}

impl Gate {
    fn new() -> Self {
        Self {
            pair: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn signal(&self) {
        let (lock, cv) = &*self.pair;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    fn wait(&self) {
        let (lock, cv) = &*self.pair;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = cv.wait(ready).unwrap();
        }
    }
}

enum ScriptStep {
    GatedOk { entered: Gate, release: Gate },
    ApprovalThenGatedOk { after_approval: Gate, release: Gate },
    Fail(String),
}

struct ScriptedRunner {
    steps: Arc<Mutex<VecDeque<ScriptStep>>>,
    approver: Arc<dyn Approver>,
}

impl PromptRunner for ScriptedRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted runner ran out of steps");
        match step {
            ScriptStep::GatedOk { entered, release } => {
                entered.signal();
                release.wait();
                emit(EntryBody::Turn {
                    n: 1,
                    text: "done".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: None,
                });
                Ok(())
            }
            ScriptStep::ApprovalThenGatedOk {
                after_approval,
                release,
            } => {
                let _ = self.approver.ask("rm -rf /tmp/demo", "clean demo");
                after_approval.signal();
                release.wait();
                emit(EntryBody::Turn {
                    n: 1,
                    text: "approved".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: None,
                });
                Ok(())
            }
            ScriptStep::Fail(msg) => Err(msg),
        }
    }
}

fn default_repl_config(repo_root: PathBuf) -> ReplConfig {
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

fn parse_surface_ready_line(line: &str) -> Option<(String, u16, String)> {
    let rest = line.strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at http://127.0.0.1:")?;
    let (port_str, token) = url.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    Some((name.to_string(), port, token.to_string()))
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
    run(&["commit", "--allow-empty", "-m", "init"]);
}

struct RunningSurface {
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    err_buf: Arc<Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<i32>,
}

impl RunningSurface {
    fn stop_and_join(self) -> (i32, String) {
        self.stop.store(true, Ordering::Relaxed);
        let code = self.handle.join().unwrap();
        let stderr = String::from_utf8_lossy(&self.err_buf.lock().unwrap()).into_owned();
        (code, stderr)
    }
}

fn start_scripted_surface(
    repo: &Path,
    name: &str,
    steps: Arc<Mutex<VecDeque<ScriptStep>>>,
) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let args = SurfaceArgs {
        port: 0,
        no_open: true,
        days: 14,
        name: Some(name.to_string()),
        resume: None,
    };
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(repo_bg);
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
            Box::new(move |_cfg, approver| {
                Ok((
                    Box::new(ScriptedRunner { steps, approver }) as Box<dyn PromptRunner>,
                    None,
                ))
            }),
        )
    });

    let deadline = Instant::now() + WAIT_DEADLINE;
    let (_name, port, token) = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    RunningSurface {
        port,
        token,
        stop,
        err_buf,
        handle,
    }
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    if let Some(tok) = token {
        req.push_str(&format!("X-Blyte-Token: {tok}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).unwrap();
    let text = String::from_utf8_lossy(&resp).into_owned();
    let (head, resp_body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response: {text:?}"));
    let status_line = head.lines().next().unwrap();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, resp_body.to_string())
}

fn get_session(port: u16, token: &str) -> serde_json::Value {
    let (status, body) = http_request(port, "GET", "/api/session", Some(token), "");
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

fn post_message(port: u16, token: &str, text: &str) {
    let payload = serde_json::json!({ "text": text }).to_string();
    let (status, body) = http_request(port, "POST", "/api/session/messages", Some(token), &payload);
    assert_eq!(status, 200, "{body}");
}

fn post_seen(port: u16, token: Option<&str>) -> u16 {
    let (status, _body) = http_request(port, "POST", "/api/session/seen", token, "");
    status
}

fn wait_for_event_kind(port: u16, token: &str, after: u64, kind: &str) -> serde_json::Value {
    let mut seq = after;
    let deadline = Instant::now() + WAIT_DEADLINE;
    while Instant::now() < deadline {
        let path = format!("/api/session/events?after={seq}&wait_ms=1000");
        let (status, body) = http_request(port, "GET", &path, Some(token), "");
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        if let Some(entries) = v["entries"].as_array() {
            for e in entries {
                if e["kind"].as_str() == Some(kind) {
                    return e.clone();
                }
            }
        }
        if let Some(last) = v["last_seq"].as_u64() {
            seq = last;
        }
    }
    panic!("timed out waiting for event kind={kind} after seq={after}");
}

fn registry_entry(reg_dir: &Path) -> RegistryEntry {
    let entries = live_entries(reg_dir).unwrap();
    assert_eq!(entries.len(), 1, "expected 1 live entry, got {entries:?}");
    entries.into_iter().next().unwrap()
}

fn assert_state(reg_dir: &Path, port: u16, token: &str, expected: SurfaceState) {
    let entry = registry_entry(reg_dir);
    assert_eq!(entry.state, expected, "registry entry state mismatch");
    let sess = get_session(port, token);
    assert_eq!(
        sess["state"].as_str(),
        Some(expected.as_str()),
        "GET /api/session state mismatch: {sess}"
    );
}

fn wait_for_state(reg_dir: &Path, port: u16, token: &str, expected: SurfaceState) {
    let deadline = Instant::now() + WAIT_DEADLINE;
    while Instant::now() < deadline {
        let entry = registry_entry(reg_dir);
        let sess = get_session(port, token);
        if entry.state == expected && sess["state"].as_str() == Some(expected.as_str()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_state(reg_dir, port, token, expected);
}

#[test]
fn surface_waiting_state_lifecycle_and_seen_endpoint() {
    let state = IsolatedStateDir::new();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("waiting-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_init_with_commit(&repo);

    let step1_entered = Gate::new();
    let step1_release = Gate::new();
    let step2_after_approval = Gate::new();
    let step2_release = Gate::new();
    let step4_entered = Gate::new();
    let step4_release = Gate::new();

    let steps = Arc::new(Mutex::new(VecDeque::from(vec![
        ScriptStep::GatedOk {
            entered: step1_entered.clone(),
            release: step1_release.clone(),
        },
        ScriptStep::ApprovalThenGatedOk {
            after_approval: step2_after_approval.clone(),
            release: step2_release.clone(),
        },
        ScriptStep::Fail("runner failed".to_string()),
        ScriptStep::GatedOk {
            entered: step4_entered.clone(),
            release: step4_release.clone(),
        },
    ])));

    let running = start_scripted_surface(&repo, "waiting-test", steps);
    let reg = state.registry_dir();

    assert_state(reg, running.port, &running.token, SurfaceState::Idle);
    assert_eq!(post_seen(running.port, None), 403);

    let mut after = get_session(running.port, &running.token)["last_seq"]
        .as_u64()
        .unwrap();
    post_message(running.port, &running.token, "first request");
    step1_entered.wait();
    assert_state(reg, running.port, &running.token, SurfaceState::Working);

    step1_release.signal();
    let idle1 = wait_for_event_kind(running.port, &running.token, after, "idle");
    after = idle1["seq"].as_u64().unwrap();
    assert_state(reg, running.port, &running.token, SurfaceState::Replied);

    assert_eq!(post_seen(running.port, Some(&running.token)), 204);
    assert_state(reg, running.port, &running.token, SurfaceState::Idle);

    post_message(running.port, &running.token, "needs approval");
    let approval_ev =
        wait_for_event_kind(running.port, &running.token, after, "approval_requested");
    let approval_id = approval_ev["id"].as_str().unwrap().to_string();
    wait_for_state(reg, running.port, &running.token, SurfaceState::Approval);

    let answer_path = format!("/api/session/approvals/{approval_id}");
    let (ans_status, ans_body) = http_request(
        running.port,
        "POST",
        &answer_path,
        Some(&running.token),
        r#"{"answer":"yes"}"#,
    );
    assert_eq!(ans_status, 200, "{ans_body}");
    step2_after_approval.wait();
    assert_state(reg, running.port, &running.token, SurfaceState::Working);

    step2_release.signal();
    let idle2 = wait_for_event_kind(running.port, &running.token, after, "idle");
    after = idle2["seq"].as_u64().unwrap();
    assert_state(reg, running.port, &running.token, SurfaceState::Replied);
    assert_eq!(post_seen(running.port, Some(&running.token)), 204);
    assert_state(reg, running.port, &running.token, SurfaceState::Idle);

    post_message(running.port, &running.token, "failing request");
    let idle3 = wait_for_event_kind(running.port, &running.token, after, "idle");
    after = idle3["seq"].as_u64().unwrap();
    assert_state(reg, running.port, &running.token, SurfaceState::Replied);
    assert_eq!(post_seen(running.port, Some(&running.token)), 204);
    assert_state(reg, running.port, &running.token, SurfaceState::Idle);

    post_message(running.port, &running.token, "seen while running");
    step4_entered.wait();
    assert_state(reg, running.port, &running.token, SurfaceState::Working);
    assert_eq!(post_seen(running.port, Some(&running.token)), 204);
    assert_state(reg, running.port, &running.token, SurfaceState::Working);

    step4_release.signal();
    let _idle4 = wait_for_event_kind(running.port, &running.token, after, "idle");
    assert_state(reg, running.port, &running.token, SurfaceState::Idle);

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}
