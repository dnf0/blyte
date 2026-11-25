use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use blyte_cli::serve::TOKEN_HEADER;

struct RecordedNotify {
    request_line: String,
    token: Option<String>,
    verdict_existed: bool,
}

fn surface_listener(verdict_path: PathBuf) -> (u16, Arc<Mutex<Vec<RecordedNotify>>>) {
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
            let verdict_existed = verdict_path.exists();
            recorded_for_thread.lock().unwrap().push(RecordedNotify {
                request_line,
                token,
                verdict_existed,
            });
            let _ = write!(
                stream,
                "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    (port, recorded)
}

fn token_path_for(repo_dir: &Path, state_home: &Path) -> PathBuf {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("XDG_STATE_HOME");
    std::env::set_var("XDG_STATE_HOME", state_home);
    let path = blyte_agent::sandbox::surface_token_path(repo_dir).unwrap();
    match prev {
        Some(v) => std::env::set_var("XDG_STATE_HOME", v),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
    path
}

fn write_surface_files(repo_dir: &Path, state_home: &Path, port: u16, token: &str) {
    let wt_dir = repo_dir.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    std::fs::write(
        wt_dir.join("surface.json"),
        format!("{{\"port\": {port}}}\n"),
    )
    .unwrap();
    let token_path = token_path_for(repo_dir, state_home);
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, format!("{token}\n")).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("README.md"), "initial\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "initial", "--no-gpg-sign"]);
    dir
}

fn seed_run(repo_dir: &Path, id: &str) {
    let wt_dir = repo_dir.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    std::fs::write(
        wt_dir.join(format!("{id}.json")),
        serde_json::json!({
            "id": id,
            "outcome": "verified",
            "exit": 0,
        })
        .to_string(),
    )
    .unwrap();
}

fn run_review_verdict(
    repo_dir: &Path,
    state_home: &Path,
    id: &str,
    verdict: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", id, "--verdict", verdict])
        .current_dir(repo_dir)
        .env("XDG_STATE_HOME", state_home)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap()
}

#[test]
fn recording_a_verdict_tells_a_running_surface() {
    let repo = init_repo();
    let state_home = tempfile::tempdir().unwrap();
    let id = "R_NOTIFY_OK";
    seed_run(repo.path(), id);

    let verdict_path = repo.path().join(".wt").join(format!("{id}.verdict.json"));
    let (port, recorded) = surface_listener(verdict_path);
    write_surface_files(repo.path(), state_home.path(), port, "review-secret-token");

    let out = run_review_verdict(repo.path(), state_home.path(), id, "accepted");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stdout.trim(), format!("recorded verdict accepted for {id}"));
    assert!(stderr.is_empty(), "expected empty stderr, got:\n{stderr}");

    let received = recorded.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].request_line, "POST /api/runs/changed HTTP/1.1");
    assert_eq!(received[0].token.as_deref(), Some("review-secret-token"));
    assert!(
        received[0].verdict_existed,
        "expected .wt/{id}.verdict.json to exist before POST /api/runs/changed arrived"
    );
}

#[test]
fn recording_a_verdict_without_a_surface_says_nothing_extra() {
    let repo = init_repo();
    let state_home = tempfile::tempdir().unwrap();
    let id = "R_NO_SURFACE";
    seed_run(repo.path(), id);

    let out = run_review_verdict(repo.path(), state_home.path(), id, "accepted");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stdout.trim(), format!("recorded verdict accepted for {id}"));
    assert!(stderr.is_empty(), "expected empty stderr, got:\n{stderr}");
}

#[test]
fn a_surface_that_cannot_be_told_does_not_undo_the_verdict() {
    let repo = init_repo();
    let state_home = tempfile::tempdir().unwrap();
    let id = "R_DEAD_SURFACE";
    seed_run(repo.path(), id);

    let closed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed_listener.local_addr().unwrap().port();
    drop(closed_listener);
    write_surface_files(
        repo.path(),
        state_home.path(),
        closed_port,
        "dead-review-token",
    );

    let out = run_review_verdict(repo.path(), state_home.path(), id, "accepted");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stdout.trim(), format!("recorded verdict accepted for {id}"));

    let verdict_path = repo.path().join(".wt").join(format!("{id}.verdict.json"));
    let raw = std::fs::read_to_string(&verdict_path)
        .unwrap_or_else(|e| panic!("expected {} to exist: {e}", verdict_path.display()));
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["verdict"], "accepted");

    let expected_prefix =
        format!("blyte: could not tell the surface at 127.0.0.1:{closed_port} that {id} finished:");
    let matching_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with(&expected_prefix))
        .collect();
    assert_eq!(
        matching_lines.len(),
        1,
        "expected one report message starting with {expected_prefix:?}, got:\n{stderr}"
    );
}
