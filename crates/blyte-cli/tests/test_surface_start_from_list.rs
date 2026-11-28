use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blyte_cli::serve::TOKEN_HEADER;

const WAIT: Duration = Duration::from_secs(30);

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

struct FakeModelServer {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeModelServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut buf = [0u8; 4096];
                        let _ = stream.read(&mut buf);
                        let body = r#"{"data":[{"id":"gpt-test"}]}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

impl Drop for FakeModelServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct SurfaceProcess {
    child: Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    name: String,
    port: u16,
    token: String,
}

impl SurfaceProcess {
    fn spawn(cwd: &Path, state_dir: &Path, home_dir: &Path, model_base_url: &str) -> Self {
        let stdout_path = state_dir.join(format!(
            "test-stdout-{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stderr_path = state_dir.join(format!(
            "test-stderr-{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(state_dir).unwrap();
        let out_file = File::create(&stdout_path).unwrap();
        let err_file = File::create(&stderr_path).unwrap();

        let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
            .arg("--no-open")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out_file))
            .stderr(Stdio::from(err_file))
            .env("HOME", home_dir)
            .env("BLYTE_STATE_DIR", state_dir)
            .env_remove("XDG_STATE_HOME")
            .env("OPENAI_BASE_URL", model_base_url)
            .env("OPENAI_API_KEY", "test-key")
            .env("BLYTE_MODEL", "openai/gpt-test")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("OLLAMA_HOST")
            .env_remove("OLLAMA_API_KEY")
            .env("BLYTE_USAGE_LOG", "off")
            .env("BLYTE_NO_UPDATE_CHECK", "1")
            .env("BLYTE_NO_KEEP_AWAKE", "1")
            .spawn()
            .expect("spawn blyte binary");

        let deadline = Instant::now() + WAIT;
        let (name, port, token) = loop {
            let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
            if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
                break parsed;
            }
            if let Some(status) = child.try_wait().unwrap() {
                let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                panic!("blyte exited early ({status}); stdout={stdout:?}; stderr={stderr:?}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for blyte surface ready line; stdout={stdout:?}; stderr={:?}",
                std::fs::read_to_string(&stderr_path).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        };

        Self {
            child,
            stdout_path,
            stderr_path,
            name,
            port,
            token,
        }
    }

    fn stdout_text(&self) -> String {
        std::fs::read_to_string(&self.stdout_path).unwrap_or_default()
    }

    fn stderr_text(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for surface process to exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for SurfaceProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct RemoteSurfaceGuard {
    port: u16,
    token: String,
    stopped: bool,
}

impl Drop for RemoteSurfaceGuard {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = try_post_json(self.port, "/api/session/stop", Some(&self.token), "{}");
        }
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

fn parse_open_url(url: &str) -> (u16, String) {
    let after_scheme = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("expected loopback url, got {url:?}"));
    let (port_str, code) = after_scheme
        .split_once("/#open=")
        .unwrap_or_else(|| panic!("expected /#open= in url, got {url:?}"));
    let port: u16 = port_str.parse().unwrap();
    (port, code.to_string())
}

fn send_http(port: u16, req: &str) -> Result<(u16, String), std::io::Error> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(35)))?;
    stream.write_all(req.as_bytes())?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp)?;
    let status = resp
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = resp
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn get_json(port: u16, path: &str, token: Option<&str>) -> (u16, serde_json::Value) {
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("{TOKEN_HEADER}: {tok}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    let (status, body) = send_http(port, &req).unwrap();
    let val = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("invalid json ({e}) for {path} (status {status}): {body:?}"));
    (status, val)
}

fn try_post_json(
    port: u16,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Result<(u16, serde_json::Value), std::io::Error> {
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(tok) = token {
        req.push_str(&format!("{TOKEN_HEADER}: {tok}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(body);
    let (status, raw_body) = send_http(port, &req)?;
    let val =
        serde_json::from_str(&raw_body).unwrap_or_else(|_| serde_json::json!({ "raw": raw_body }));
    Ok((status, val))
}

fn post_json(port: u16, path: &str, token: Option<&str>, body: &str) -> (u16, serde_json::Value) {
    try_post_json(port, path, token, body).unwrap()
}

fn wait_until_health_fails(port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let req = format!(
            "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        if send_http(port, &req).is_err() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for surface on port {port} to stop answering /api/health"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_until_removed(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {} to be removed",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn start_surface_in_another_folder_outlives_first_and_stops_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("state");
    let home_dir = tmp.path().join("home");
    let repo_a = tmp.path().join("repo-a");
    let repo_b = tmp.path().join("repo-b");
    std::fs::create_dir_all(&home_dir).unwrap();
    init_git_repo(&repo_a);
    init_git_repo(&repo_b);

    let fake_model = FakeModelServer::start();
    let mut first = SurfaceProcess::spawn(&repo_a, &state_dir, &home_dir, &fake_model.base_url());

    let (no_tok_status, _) = post_json(
        first.port,
        "/api/surfaces/start",
        None,
        &serde_json::json!({ "folder": repo_b.to_string_lossy() }).to_string(),
    );
    assert_eq!(no_tok_status, 403);

    let (start_status, start_body) = post_json(
        first.port,
        "/api/surfaces/start",
        Some(&first.token),
        &serde_json::json!({ "folder": repo_b.to_string_lossy() }).to_string(),
    );
    assert_eq!(start_status, 200, "start failed: {start_body}");
    let new_name = start_body["name"].as_str().unwrap().to_string();
    let new_url = start_body["url"].as_str().unwrap().to_string();
    assert!(!new_name.is_empty());
    assert!(
        new_url.contains("/#open="),
        "expected #open= url: {new_url}"
    );
    assert!(
        !new_url.contains("#token="),
        "url must not contain #token=: {new_url}"
    );

    let (new_port, open_code) = parse_open_url(&new_url);

    let (health_status, health_body) = get_json(new_port, "/api/health", None);
    assert_eq!(
        health_status, 200,
        "new surface health failed: {health_body}"
    );
    let new_surface_id = health_body["surface_id"].as_str().unwrap().to_string();

    let (exchange_status, exchange_body) = post_json(
        new_port,
        "/api/open",
        None,
        &serde_json::json!({ "code": open_code }).to_string(),
    );
    assert_eq!(exchange_status, 200, "exchange failed: {exchange_body}");
    let new_token = exchange_body["token"].as_str().unwrap().to_string();
    assert!(!new_token.is_empty());

    let mut second_guard = RemoteSurfaceGuard {
        port: new_port,
        token: new_token.clone(),
        stopped: false,
    };

    let (session_status, session_body) = get_json(new_port, "/api/session", Some(&new_token));
    assert_eq!(
        session_status, 200,
        "new surface /api/session failed: {session_body}"
    );
    assert_eq!(session_body["surface_name"], new_name);

    let reply_raw = serde_json::to_string(&start_body).unwrap();
    assert!(
        !reply_raw.contains(&new_token),
        "start reply leaked new surface token"
    );
    assert!(
        !first.stdout_text().contains(&new_token),
        "first surface stdout leaked new surface token"
    );
    assert!(
        !first.stderr_text().contains(&new_token),
        "first surface stderr leaked new surface token"
    );

    let start_logs_dir = state_dir.join("surface-starts");
    assert!(
        start_logs_dir.is_dir(),
        "expected start logs dir at {}",
        start_logs_dir.display()
    );
    let remaining_logs: Vec<_> = std::fs::read_dir(&start_logs_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        remaining_logs.is_empty(),
        "start log files were not removed after successful start: {remaining_logs:?}"
    );

    let first_health = get_json(first.port, "/api/health", None).1;
    let first_surface_id = first_health["surface_id"].as_str().unwrap().to_string();
    let first_registry_path = state_dir
        .join("surfaces")
        .join(format!("{first_surface_id}.json"));
    let first_token_path = state_dir
        .join("surface")
        .join(format!("{first_surface_id}.token"));
    assert!(first_registry_path.exists());
    assert!(first_token_path.exists());

    let (unauth_stop_status, _) = post_json(first.port, "/api/session/stop", None, "{}");
    assert_eq!(unauth_stop_status, 403);
    assert_eq!(get_json(first.port, "/api/health", None).0, 200);

    let (stop_first_status, _) =
        post_json(first.port, "/api/session/stop", Some(&first.token), "{}");
    assert_eq!(stop_first_status, 202);
    let first_exit = first.wait_for_exit(WAIT);
    assert!(
        first_exit.success(),
        "first surface exited with {first_exit}"
    );
    let first_stdout = first.stdout_text();
    let expected_resume = format!("resume with: blyte --resume {}", first.name);
    assert!(
        first_stdout.contains(&expected_resume),
        "expected {expected_resume:?} in first surface stdout: {first_stdout:?}"
    );
    assert!(
        !first_registry_path.exists(),
        "first surface registry entry still exists at {}",
        first_registry_path.display()
    );
    assert!(
        !first_token_path.exists(),
        "first surface token file still exists at {}",
        first_token_path.display()
    );

    let (still_alive_status, still_alive_body) = get_json(new_port, "/api/health", None);
    assert_eq!(still_alive_status, 200);
    assert_eq!(still_alive_body["surface_id"], new_surface_id);

    let new_registry_path = state_dir
        .join("surfaces")
        .join(format!("{new_surface_id}.json"));
    let new_token_path = state_dir
        .join("surface")
        .join(format!("{new_surface_id}.token"));
    assert!(new_registry_path.exists());
    assert!(new_token_path.exists());

    let (stop_second_status, _) = post_json(new_port, "/api/session/stop", Some(&new_token), "{}");
    assert_eq!(stop_second_status, 202);
    second_guard.stopped = true;
    wait_until_health_fails(new_port, WAIT);
    wait_until_removed(&new_registry_path, WAIT);
    wait_until_removed(&new_token_path, WAIT);
}

#[test]
fn start_surface_validation_errors_and_early_child_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("state");
    let home_dir = tmp.path().join("home");
    let repo_a = tmp.path().join("repo-a");
    std::fs::create_dir_all(&home_dir).unwrap();
    init_git_repo(&repo_a);

    let fake_model = FakeModelServer::start();
    let mut first = SurfaceProcess::spawn(&repo_a, &state_dir, &home_dir, &fake_model.base_url());

    let entries_before = std::fs::read_dir(state_dir.join("surfaces"))
        .unwrap()
        .count();

    let missing_path = tmp.path().join("does-not-exist");
    let (status_missing, body_missing) = post_json(
        first.port,
        "/api/surfaces/start",
        Some(&first.token),
        &serde_json::json!({ "folder": missing_path.to_string_lossy() }).to_string(),
    );
    assert_eq!(status_missing, 400, "body={body_missing}");
    let msg_missing = body_missing["error"].as_str().unwrap();
    assert!(
        msg_missing.contains(&missing_path.display().to_string()),
        "{msg_missing}"
    );
    assert!(msg_missing.contains("does not exist"), "{msg_missing}");
    assert!(!missing_path.exists());

    let regular_file = tmp.path().join("plain.txt");
    std::fs::write(&regular_file, "hi\n").unwrap();
    let (status_file, body_file) = post_json(
        first.port,
        "/api/surfaces/start",
        Some(&first.token),
        &serde_json::json!({ "folder": regular_file.to_string_lossy() }).to_string(),
    );
    assert_eq!(status_file, 400, "body={body_file}");
    let msg_file = body_file["error"].as_str().unwrap();
    assert!(
        msg_file.contains(&regular_file.display().to_string()),
        "{msg_file}"
    );
    assert!(msg_file.contains("not a directory"), "{msg_file}");

    let (status_rel, body_rel) = post_json(
        first.port,
        "/api/surfaces/start",
        Some(&first.token),
        &serde_json::json!({ "folder": "relative/path" }).to_string(),
    );
    assert_eq!(status_rel, 400, "body={body_rel}");
    let msg_rel = body_rel["error"].as_str().unwrap();
    assert!(msg_rel.contains("relative/path"), "{msg_rel}");
    assert!(msg_rel.contains("absolute"), "{msg_rel}");

    let entries_after = std::fs::read_dir(state_dir.join("surfaces"))
        .unwrap()
        .count();
    assert_eq!(entries_before, entries_after);

    let broken_repo = tmp.path().join("broken-repo");
    std::fs::create_dir_all(&broken_repo).unwrap();
    std::fs::write(
        broken_repo.join(".git"),
        "gitdir: /nonexistent/blyte-broken-gitdir\n",
    )
    .unwrap();
    let (status_exit, body_exit) = post_json(
        first.port,
        "/api/surfaces/start",
        Some(&first.token),
        &serde_json::json!({ "folder": broken_repo.to_string_lossy() }).to_string(),
    );
    assert_eq!(
        status_exit, 500,
        "expected 500 on early child exit: {body_exit}"
    );
    let msg_exit = body_exit["error"].as_str().unwrap();
    assert!(
        msg_exit.contains("exited") && msg_exit.contains("blyte:"),
        "expected child output in 500 error: {msg_exit:?}"
    );

    let start_logs_dir = state_dir.join("surface-starts");
    assert!(
        start_logs_dir.is_dir(),
        "expected start logs dir at {}",
        start_logs_dir.display()
    );
    let remaining_logs: Vec<_> = std::fs::read_dir(&start_logs_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        remaining_logs.is_empty(),
        "start log files were not removed after early child exit: {remaining_logs:?}"
    );

    let (stop_status, _) = post_json(first.port, "/api/session/stop", Some(&first.token), "{}");
    assert_eq!(stop_status, 202);
    let first_exit = first.wait_for_exit(WAIT);
    assert!(first_exit.success());
}
