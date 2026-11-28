use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use blyte_cli::cli::SurfaceArgs;
use blyte_cli::http::Request;
use blyte_cli::open_window::{
    open_command, resolve_open_in, resolve_open_in_os, OpenIn, OpenPlan, APP_FALLBACK_NOTICE,
    OPEN_IN_ENV,
};
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{generate_token, route, run_with_hooks, OpenCodes, Surface, TOKEN_HEADER};
use blyte_cli::session::{EntryBody, PromptRunner, Session};

const WAIT: Duration = Duration::from_secs(30);

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
    static ISOLATED: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = ISOLATED.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("XDG_STATE_HOME", dir.path());
    std::env::set_var("BLYTE_STATE_DIR", dir.path());
}

fn make_surface(root: PathBuf) -> Surface {
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    let session = Session::start(Box::new(NoopRunner), "gemini-4-neon".to_string());
    Surface {
        surface_id: "s00000000000000aa".to_string(),
        surface_name: repo.clone(),
        root,
        repo,
        session,
        session_id: "sess-open-codes".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        message_credential: generate_token().unwrap(),
        open_codes: OpenCodes::new(),
        requests: Arc::new(blyte_cli::requests::RequestsState::new()),
        publisher: Arc::new(blyte_cli::requests::RealPublisher),
        reconcile_lock: Mutex::new(()),
        reconcile_requested: AtomicBool::new(false),
    }
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

fn post_request(path: &str, headers: Vec<(&str, &str)>, body: &[u8]) -> Request {
    Request {
        method: "POST".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.to_vec(),
    }
}

fn send_http(port: u16, raw_request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(raw_request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

#[test]
fn open_command_macos_app_prefers_browsers_in_order() {
    let url = "http://127.0.0.1:4100/#open=abc123?x=1&y=2";
    let expected_order = [
        "Google Chrome",
        "Chromium",
        "Microsoft Edge",
        "Brave Browser",
    ];
    for (idx, chosen) in expected_order.iter().enumerate() {
        let allowed = &expected_order[idx..];
        let plan =
            open_command(OpenIn::App, url, "macos", &|name| allowed.contains(&name)).expect("plan");
        assert_eq!(
            plan,
            OpenPlan {
                program: "open".to_string(),
                args: vec![
                    "-na".to_string(),
                    (*chosen).to_string(),
                    "--args".to_string(),
                    format!("--app={url}"),
                ],
                notice: None,
            }
        );
    }
}

#[test]
fn open_command_linux_app_prefers_browsers_in_order() {
    let url = "http://127.0.0.1:4100/#open=abc123?x=1&y=2";
    let expected_order = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
    ];
    for (idx, chosen) in expected_order.iter().enumerate() {
        let allowed = &expected_order[idx..];
        let plan =
            open_command(OpenIn::App, url, "linux", &|name| allowed.contains(&name)).expect("plan");
        assert_eq!(
            plan,
            OpenPlan {
                program: (*chosen).to_string(),
                args: vec![format!("--app={url}")],
                notice: None,
            }
        );
    }
}

#[test]
fn open_command_app_with_none_installed_falls_back_with_notice() {
    let url = "http://127.0.0.1:4100/#open=deadbeef";
    let mac = open_command(OpenIn::App, url, "macos", &|_| false).expect("mac plan");
    assert_eq!(
        mac,
        OpenPlan {
            program: "open".to_string(),
            args: vec![url.to_string()],
            notice: Some(APP_FALLBACK_NOTICE.to_string()),
        }
    );

    let linux = open_command(OpenIn::App, url, "linux", &|_| false).expect("linux plan");
    assert_eq!(
        linux,
        OpenPlan {
            program: "xdg-open".to_string(),
            args: vec![url.to_string()],
            notice: Some(APP_FALLBACK_NOTICE.to_string()),
        }
    );
}

#[test]
fn open_command_browser_uses_default_without_notice_and_preserves_url_arg() {
    let url = "http://127.0.0.1:4100/#open=abc%20def&x=1";
    let mac = open_command(OpenIn::Browser, url, "macos", &|_| true).expect("mac plan");
    assert_eq!(
        mac,
        OpenPlan {
            program: "open".to_string(),
            args: vec![url.to_string()],
            notice: None,
        }
    );

    let linux = open_command(OpenIn::Browser, url, "linux", &|_| true).expect("linux plan");
    assert_eq!(
        linux,
        OpenPlan {
            program: "xdg-open".to_string(),
            args: vec![url.to_string()],
            notice: None,
        }
    );
}

#[test]
fn resolve_open_in_defaults_precedence_and_errors() {
    assert_eq!(resolve_open_in(None, None).unwrap(), OpenIn::App);
    assert_eq!(
        resolve_open_in(None, Some("browser")).unwrap(),
        OpenIn::Browser
    );
    assert_eq!(resolve_open_in(None, Some("app")).unwrap(), OpenIn::App);
    assert_eq!(
        resolve_open_in(Some("app"), Some("browser")).unwrap(),
        OpenIn::App
    );
    assert_eq!(
        resolve_open_in(Some("browser"), Some("invalid")).unwrap(),
        OpenIn::Browser
    );

    let flag_err = resolve_open_in(Some("x"), None).unwrap_err();
    assert!(flag_err.contains("--open-in"), "{flag_err}");
    assert!(flag_err.contains('x'), "{flag_err}");

    let env_err = resolve_open_in(None, Some("x")).unwrap_err();
    assert!(env_err.contains(OPEN_IN_ENV), "{env_err}");
    assert!(env_err.contains('x'), "{env_err}");
}

#[cfg(unix)]
#[test]
fn resolve_open_in_rejects_non_utf8_env_value() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let raw = OsStr::from_bytes(b"bad\xff");
    let err = resolve_open_in_os(None, Some(raw)).unwrap_err();
    assert!(err.contains(OPEN_IN_ENV), "{err}");
    assert!(err.contains("bad"), "{err}");

    assert_eq!(
        resolve_open_in_os(Some("browser"), Some(raw)).unwrap(),
        OpenIn::Browser
    );

    let cli_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .env(OPEN_IN_ENV, raw)
        .output()
        .unwrap();
    assert_eq!(cli_out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&cli_out.stderr);
    assert!(stderr.contains(OPEN_IN_ENV), "{stderr}");
    assert!(stderr.contains("bad"), "{stderr}");

    let open_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["open", "demo"])
        .env(OPEN_IN_ENV, raw)
        .output()
        .unwrap();
    assert_eq!(open_out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&open_out.stderr);
    assert!(stderr.contains(OPEN_IN_ENV), "{stderr}");
    assert!(stderr.contains("bad"), "{stderr}");
}

#[test]
fn cli_open_in_setting_validation_and_conflicts_exit_2() {
    let bad_flag = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["--open-in", "x"])
        .env_remove(OPEN_IN_ENV)
        .output()
        .unwrap();
    assert_eq!(bad_flag.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad_flag.stderr);
    assert!(stderr.contains("--open-in"), "{stderr}");
    assert!(stderr.contains('x'), "{stderr}");

    let bad_env = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .env(OPEN_IN_ENV, "x")
        .output()
        .unwrap();
    assert_eq!(bad_env.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad_env.stderr);
    assert!(stderr.contains(OPEN_IN_ENV), "{stderr}");
    assert!(stderr.contains('x'), "{stderr}");

    let bad_open_flag = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["open", "demo", "--open-in", "x"])
        .env_remove(OPEN_IN_ENV)
        .output()
        .unwrap();
    assert_eq!(bad_open_flag.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad_open_flag.stderr);
    assert!(stderr.contains("--open-in"), "{stderr}");
    assert!(stderr.contains('x'), "{stderr}");

    let bad_open_env = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["open", "demo"])
        .env(OPEN_IN_ENV, "x")
        .output()
        .unwrap();
    assert_eq!(bad_open_env.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad_open_env.stderr);
    assert!(stderr.contains(OPEN_IN_ENV), "{stderr}");
    assert!(stderr.contains('x'), "{stderr}");

    let flag_wins_over_bad_env = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["open", "nonexistent-surface-xyz", "--open-in", "browser"])
        .env(OPEN_IN_ENV, "x")
        .env("BLYTE_STATE_DIR", tempfile::tempdir().unwrap().path())
        .output()
        .unwrap();
    assert_eq!(flag_wins_over_bad_env.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&flag_wins_over_bad_env.stderr);
    assert!(
        stderr.contains("no running surface named nonexistent-surface-xyz"),
        "{stderr}"
    );

    let with_terminal = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["--terminal", "--open-in", "browser"])
        .env_remove(OPEN_IN_ENV)
        .output()
        .unwrap();
    assert_eq!(with_terminal.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&with_terminal.stderr);
    assert!(
        stderr.contains("--open-in") && stderr.contains("--terminal"),
        "{stderr}"
    );

    let with_prompt = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["--open-in", "browser", "hello"])
        .env_remove(OPEN_IN_ENV)
        .output()
        .unwrap();
    assert_eq!(with_prompt.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&with_prompt.stderr);
    assert!(stderr.contains("--open-in"), "{stderr}");
}

#[test]
fn open_codes_routes_auth_single_use_expiry_and_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let surface = make_surface(dir.path().to_path_buf());
    let t0 = 10_000u64;

    let unauth_create = route(
        &surface,
        &post_request("/api/session/open-codes", vec![], b"{}"),
        t0,
    );
    assert_eq!(unauth_create.status, 403);

    let create_resp = route(
        &surface,
        &post_request(
            "/api/session/open-codes",
            vec![(TOKEN_HEADER, &surface.token)],
            b"{}",
        ),
        t0,
    );
    assert_eq!(create_resp.status, 200);
    let created: serde_json::Value = serde_json::from_slice(&create_resp.body).unwrap();
    let code = created["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 64);
    assert!(code.chars().all(|c| c.is_ascii_hexdigit()));

    let exchange = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": code }).to_string().as_bytes(),
        ),
        t0,
    );
    assert_eq!(exchange.status, 200);
    let exchanged: serde_json::Value = serde_json::from_slice(&exchange.body).unwrap();
    assert_eq!(exchanged["token"].as_str(), Some(surface.token.as_str()));

    let second_use = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": code }).to_string().as_bytes(),
        ),
        t0,
    );
    assert_eq!(second_use.status, 403);
    assert_eq!(second_use.body, b"{}");

    let unknown = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": "not-a-real-code" })
                .to_string()
                .as_bytes(),
        ),
        t0,
    );
    assert_eq!(unknown.status, 403);
    assert_eq!(unknown.body, b"{}");

    let expiring_code = surface.open_codes.issue(t0).unwrap();
    let at_60s = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": expiring_code })
                .to_string()
                .as_bytes(),
        ),
        t0 + 60,
    );
    assert_eq!(at_60s.status, 200);

    let expired_code = surface.open_codes.issue(t0).unwrap();
    let after_60s = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": expired_code })
                .to_string()
                .as_bytes(),
        ),
        t0 + 61,
    );
    assert_eq!(after_60s.status, 403);
    assert_eq!(after_60s.body, b"{}");

    let mut codes = Vec::new();
    for i in 0..17u64 {
        codes.push(surface.open_codes.issue(t0 + i).unwrap());
    }
    let evicted = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": codes[0] })
                .to_string()
                .as_bytes(),
        ),
        t0 + 20,
    );
    assert_eq!(evicted.status, 403);
    assert_eq!(evicted.body, b"{}");

    let still_present = route(
        &surface,
        &post_request(
            "/api/open",
            vec![],
            serde_json::json!({ "code": codes[1] })
                .to_string()
                .as_bytes(),
        ),
        t0 + 20,
    );
    assert_eq!(still_present.status, 200);
}

#[test]
fn real_surface_opens_with_open_code_never_token_and_enforces_origin_on_api_open() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("open-code-surface");
    std::fs::create_dir_all(&repo).unwrap();

    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let stop = Arc::new(AtomicBool::new(false));

    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let opened_bg = Arc::clone(&opened);
    let stop_bg = Arc::clone(&stop);
    let repo_bg = repo.clone();

    let handle = std::thread::spawn(move || {
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = move |url: &str| {
            opened_bg.lock().unwrap().push(url.to_string());
        };
        let args = SurfaceArgs {
            port: 0,
            no_open: false,
            days: 14,
            name: None,
            resume: None,
            open_in: OpenIn::App,
        };
        let cfg = default_repl_config(repo_bg);
        run_with_hooks(&args, cfg, &mut out, &mut err, &mut opener, stop_bg)
    });

    let deadline = Instant::now() + WAIT;
    let (port, token) = loop {
        let stdout = String::from_utf8_lossy(&out_buf.lock().unwrap()).to_string();
        if let Some(line) = stdout.lines().find(|l| l.starts_with("blyte: surface ")) {
            let rest = line.strip_prefix("blyte: surface ").unwrap();
            let (_, url) = rest.split_once(" at http://127.0.0.1:").unwrap();
            let (port_str, tok) = url.split_once("/#token=").unwrap();
            let port: u16 = port_str.parse().unwrap();
            break (port, tok.to_string());
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for surface start line; stdout={:?}; stderr={:?}",
            stdout,
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    let recorded_url = loop {
        if let Some(url) = opened.lock().unwrap().first().cloned() {
            break url;
        }
        assert!(Instant::now() < deadline, "timed out waiting for opener");
        std::thread::sleep(Duration::from_millis(10));
    };

    let prefix = format!("http://127.0.0.1:{port}/#open=");
    let code = recorded_url.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("expected opener URL starting with {prefix}, got {recorded_url}")
    });
    assert!(!code.is_empty());
    assert!(!recorded_url.contains(&token));
    assert!(!recorded_url.contains("#token="));

    let body = serde_json::json!({ "code": code }).to_string();
    let other_port = if port == 9999 { 9998 } else { 9999 };
    let bad_origin_resp = send_http(
        port,
        &format!(
            "POST /api/open HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{other_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert!(
        bad_origin_resp.starts_with("HTTP/1.1 403"),
        "expected 403 for cross-port origin, got: {bad_origin_resp}"
    );

    let ok_resp = send_http(
        port,
        &format!(
            "POST /api/open HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert!(ok_resp.starts_with("HTTP/1.1 200"), "resp: {ok_resp}");
    let (_, json_body) = ok_resp.split_once("\r\n\r\n").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(json_body).unwrap();
    assert_eq!(parsed["token"].as_str(), Some(token.as_str()));

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);
}
