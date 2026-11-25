use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use blyte_cli::serve::TOKEN_HEADER;

const TOOL_CALL_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"true\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
const COMMIT_CALL_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"echo ok > change.txt && git add change.txt && git commit -m change --no-gpg-sign\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
const DONE_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

struct RecordedNotify {
    request_line: String,
    token: Option<String>,
    summary_existed: bool,
}

fn model_server_with<F>(handler: F) -> (u16, Arc<AtomicUsize>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let body = handler(n);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    (port, requests)
}

fn surface_listener(summary_path: PathBuf) -> (u16, Arc<Mutex<Vec<RecordedNotify>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let recorded_for_thread = Arc::clone(&recorded);
    let token_header_lower = format!("{}:", TOKEN_HEADER.to_ascii_lowercase());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = vec![0u8; 8192];
            let mut got = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                if got.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&got);
            let mut lines = text.lines();
            let request_line = lines
                .next()
                .unwrap_or("")
                .trim_end_matches('\r')
                .to_string();
            let token = lines.find_map(|line| {
                let trimmed = line.trim_end_matches('\r');
                let lower = trimmed.to_ascii_lowercase();
                if lower.starts_with(&token_header_lower) {
                    Some(trimmed[token_header_lower.len()..].trim().to_string())
                } else {
                    None
                }
            });
            let summary_existed = summary_path.exists();
            recorded_for_thread.lock().unwrap().push(RecordedNotify {
                request_line,
                token,
                summary_existed,
            });
            let _ = write!(
                stream,
                "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    (port, recorded)
}

fn state_home() -> &'static Path {
    static STATE_HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE_HOME
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("XDG_STATE_HOME", dir.path());
            dir
        })
        .path()
}

fn write_surface_files(repo_dir: &Path, surface_id: &str, port: u16, token: &str) {
    let _ = state_home();
    blyte_cli::surface_dir::write_record(
        repo_dir,
        &blyte_cli::surface_dir::SurfaceRecord {
            id: surface_id.to_string(),
            name: "demo".to_string(),
            pid: std::process::id(),
            port,
            session_id: format!("session-{surface_id}"),
            started_at: 1_700_000_000,
        },
    )
    .unwrap();
    let token_path = blyte_agent::sandbox::surface_token_path_for_id(surface_id).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();
}

fn dispatch_command(repo_dir: &Path, home: &Path, port: u16) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.current_dir(repo_dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_STATE_HOME", state_home())
        .env("BLYTE_STATE_DIR", home.join("state"))
        .env("BLYTE_GATE_SLOT_DIR", home.join("slots"))
        .env("BLYTE_GATE_SLOTS", "16")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/gemini-3.8-flash");
    cmd
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn base_repo_template() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path();
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(p)
                .output()
                .unwrap();
            Command::new("git")
                .args(["config", "user.email", "test@example.com"])
                .current_dir(p)
                .output()
                .unwrap();
            Command::new("git")
                .args(["config", "user.name", "Test User"])
                .current_dir(p)
                .output()
                .unwrap();
            std::fs::write(p.join("README.md"), "initial").unwrap();
            Command::new("git")
                .args(["add", "."])
                .current_dir(p)
                .output()
                .unwrap();
            Command::new("git")
                .args([
                    "-c",
                    "maintenance.auto=false",
                    "-c",
                    "gc.auto=0",
                    "commit",
                    "-m",
                    "initial",
                    "--no-gpg-sign",
                ])
                .current_dir(p)
                .output()
                .unwrap();
            dir
        })
        .path()
}

fn init_repo(dir: &Path) {
    copy_dir_all(base_repo_template(), dir);
}

fn run_verified_dispatch(
    repo_dir: &Path,
    home: &Path,
    id: &str,
    surface: Option<&str>,
) -> std::process::Output {
    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Make a small change").unwrap();

    let (model_port, _requests) = model_server_with(|n| {
        if n == 1 {
            COMMIT_CALL_BODY.to_string()
        } else {
            DONE_BODY.to_string()
        }
    });

    let mut cmd = dispatch_command(repo_dir, home, model_port);
    cmd.args([
        "dispatch",
        id,
        task_file.to_str().unwrap(),
        "--gate",
        "exit 0",
        "--sandbox",
        "off",
    ]);
    if let Some(sid) = surface {
        cmd.args(["--surface", sid]);
    }
    cmd.output().unwrap()
}

#[test]
fn dispatch_tells_the_surface_when_it_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let id = "T_NOTIFY_OK";
    let sid = "sa1b2c3d4e5f60718";
    let summary_path = repo_dir.join(".wt").join(format!("{id}.json"));
    let (surface_port, recorded) = surface_listener(summary_path);
    write_surface_files(repo_dir, sid, surface_port, "secret-token-123");

    let out = run_verified_dispatch(repo_dir, home.path(), id, Some(sid));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );

    let summary: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON stdout: {e}\nstdout: {stdout}\nstderr: {stderr}"));
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["exit"], 0);

    let received = recorded.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].request_line, "POST /api/runs/changed HTTP/1.1");
    assert_eq!(received[0].token.as_deref(), Some("secret-token-123"));
    assert!(
        received[0].summary_existed,
        "expected .wt/{id}.json to exist before POST /api/runs/changed arrived"
    );
}

#[test]
fn dispatch_without_a_surface_prints_nothing_about_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let out = run_verified_dispatch(repo_dir, home.path(), "T_NO_SURFACE", None);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );

    let summary: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON stdout: {e}\nstdout: {stdout}\nstderr: {stderr}"));
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["exit"], 0);
    assert!(
        !stderr.to_ascii_lowercase().contains("surface"),
        "expected stderr not to mention surface, got:\n{stderr}"
    );
}

#[test]
fn a_dead_surface_does_not_change_the_result() {
    let plain_temp = tempfile::tempdir().unwrap();
    let plain_home = tempfile::tempdir().unwrap();
    init_repo(plain_temp.path());
    let plain_out = run_verified_dispatch(plain_temp.path(), plain_home.path(), "T_DEAD_REF", None);
    let plain_summary: serde_json::Value = serde_json::from_slice(&plain_out.stdout).unwrap();

    let dead_temp = tempfile::tempdir().unwrap();
    let dead_home = tempfile::tempdir().unwrap();
    let dead_repo = dead_temp.path();
    init_repo(dead_repo);

    let sid = "sdead111122223333";
    let closed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed_listener.local_addr().unwrap().port();
    drop(closed_listener);
    write_surface_files(dead_repo, sid, closed_port, "dead-surface-token");

    let dead_out = run_verified_dispatch(dead_repo, dead_home.path(), "T_DEAD_SURFACE", Some(sid));
    let dead_stdout = String::from_utf8_lossy(&dead_out.stdout);
    let dead_stderr = String::from_utf8_lossy(&dead_out.stderr);
    let dead_summary: serde_json::Value = serde_json::from_str(&dead_stdout).unwrap_or_else(|e| {
        panic!("invalid JSON stdout: {e}\nstdout: {dead_stdout}\nstderr: {dead_stderr}")
    });

    assert_eq!(dead_out.status.code(), plain_out.status.code());
    assert_eq!(dead_summary["outcome"], plain_summary["outcome"]);
    assert_eq!(dead_summary["exit"], plain_summary["exit"]);

    let matching_lines: Vec<&str> = dead_stderr
        .lines()
        .filter(|line| line.starts_with("blyte: could not tell the surface"))
        .collect();
    assert_eq!(
        matching_lines.len(),
        1,
        "expected exactly one surface warning line in stderr, got:\n{dead_stderr}"
    );
}

#[test]
fn a_blocked_run_tells_the_surface_too() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let id = "T_BLOCKED_NOTIFY";
    let sid = "sb10c4ed000001234";
    let summary_path = repo_dir.join(".wt").join(format!("{id}.json"));
    let (surface_port, recorded) = surface_listener(summary_path);
    write_surface_files(repo_dir, sid, surface_port, "blocked-token-456");

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let (model_port, _requests) = model_server_with(|_| TOOL_CALL_BODY.to_string());
    let out = dispatch_command(repo_dir, home.path(), model_port)
        .args([
            "dispatch",
            id,
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--sandbox",
            "off",
            "--surface",
            sid,
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "stdout: {stdout}\nstderr: {stderr}"
    );

    let summary: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON stdout: {e}\nstdout: {stdout}\nstderr: {stderr}"));
    assert_eq!(summary["outcome"], "blocked");
    assert_eq!(summary["exit"], 5);

    let received = recorded.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].request_line, "POST /api/runs/changed HTTP/1.1");
    assert_eq!(received[0].token.as_deref(), Some("blocked-token-456"));
    assert!(
        received[0].summary_existed,
        "expected .wt/{id}.json to exist before POST /api/runs/changed arrived"
    );
}
