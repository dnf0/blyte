use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_cli::cli::SurfaceArgs;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{run_with_hooks, TOKEN_HEADER};
use blyte_cli::session::{Entry, EntryBody};
use blyte_cli::surface_dir::surface_dir;

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

fn default_repl_config(repo: PathBuf) -> ReplConfig {
    ReplConfig {
        repo_root: repo,
        gate: None,
        context_strategy: "folded".to_string(),
        max_turns: 4,
        max_gates: 1,
        rollback: false,
        model: Some("openai/gpt-test".to_string()),
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

fn setup_test_env(base_dir: &Path) -> EnvRestore {
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
    std::env::set_var("OPENAI_BASE_URL", "http://127.0.0.1:1/v1");
    std::env::set_var("OPENAI_API_KEY", "test-key");
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
    restore
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
        .set_read_timeout(Some(Duration::from_secs(30)))
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

fn fetch_surfaces(port: u16, token: &str) -> (String, serde_json::Value) {
    let resp = send_http(
        port,
        &format!(
            "GET /api/surfaces HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nConnection: close\r\n\r\n"
        ),
    );
    let status_line = resp.lines().next().unwrap_or("").to_string();
    let body = http_json_body(&resp);
    (status_line, body)
}

struct RunningSurface {
    name: String,
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    out_buf: Arc<Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<i32>,
}

impl RunningSurface {
    fn stop_and_join(self) -> (i32, String) {
        self.stop.store(true, Ordering::Relaxed);
        let code = self.handle.join().unwrap();
        let out = String::from_utf8_lossy(&self.out_buf.lock().unwrap()).into_owned();
        (code, out)
    }
}

fn start_surface(repo: &Path, args: SurfaceArgs) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(repo_bg);
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
        handle,
    }
}

#[test]
fn clean_stop_prints_resume_line_and_failed_start_prints_none() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);
    let _env = setup_test_env(tmp.path());

    let running = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
    );
    let name = running.name.clone();
    let (code, stdout) = running.stop_and_join();
    assert_eq!(code, 0);
    let expected = format!("blyte: stopped surface {name}; resume with: blyte --resume {name}\n");
    assert!(
        stdout.ends_with(&expected),
        "expected stdout to end with {expected:?}, got {stdout:?}"
    );

    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied_port = occupied.local_addr().unwrap().port();
    let mut fail_out = Vec::new();
    let mut fail_err = Vec::new();
    let mut opener = |_: &str| {};
    let fail_code = run_with_hooks(
        &SurfaceArgs {
            no_open: true,
            port: occupied_port,
            ..Default::default()
        },
        default_repl_config(repo.clone()),
        &mut fail_out,
        &mut fail_err,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    drop(occupied);
    assert_eq!(fail_code, 1);
    let fail_stdout = String::from_utf8_lossy(&fail_out);
    assert!(
        !fail_stdout.contains("stopped surface"),
        "failed start must not print a stop line, got {fail_stdout:?}"
    );
    assert!(
        !fail_stdout.contains("blyte --resume"),
        "failed start must not print a resume command, got {fail_stdout:?}"
    );
}

#[test]
fn api_surfaces_lists_stopped_surface_for_folder_and_excludes_running_surface() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_git_repo(&repo);
    let _env = setup_test_env(tmp.path());

    let first = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
    );
    let stopped_name = first.name.clone();
    let stopped_id = fetch_health(first.port)["surface_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _) = first.stop_and_join();
    assert_eq!(code, 0);

    let stopped_entries_path = surface_dir(&repo, &stopped_id).join("entries.jsonl");
    let entry = Entry {
        seq: 1,
        ts: 1_700_000_123,
        body: EntryBody::UserMessage {
            text: "fix login redirect\nmore detail".to_string(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stopped_entries_path)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&entry).unwrap()).unwrap();
    drop(file);

    let running = start_surface(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
    );
    let running_id = fetch_health(running.port)["surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status_line, body) = fetch_surfaces(running.port, &running.token);
    assert!(
        status_line.starts_with("HTTP/1.1 200"),
        "expected 200 OK, got {status_line:?} body={body}"
    );

    let stopped = body["stopped"].as_array().expect("stopped array");
    assert_eq!(stopped.len(), 1, "expected 1 stopped surface: {body}");
    assert_eq!(stopped[0]["id"], stopped_id);
    assert_eq!(stopped[0]["name"], stopped_name);
    assert_eq!(stopped[0]["last_used"], 1_700_000_123);
    assert_eq!(stopped[0]["last_message"], "fix login redirect");
    assert!(
        !stopped.iter().any(|s| s["id"] == running_id),
        "running surface must not appear in stopped: {body}"
    );

    std::fs::write(&stopped_entries_path, "not valid json\n").unwrap();
    let (err_status, err_body) = fetch_surfaces(running.port, &running.token);
    assert!(
        err_status.starts_with("HTTP/1.1 500"),
        "expected 500 on unreadable stopped entries, got {err_status:?} body={err_body}"
    );
    assert!(
        err_body["error"]
            .as_str()
            .unwrap_or("")
            .contains("invalid entry"),
        "expected error detail in 500 response: {err_body}"
    );

    let (running_code, _) = running.stop_and_join();
    assert_eq!(running_code, 0);
}
