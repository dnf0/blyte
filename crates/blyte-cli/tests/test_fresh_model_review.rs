//! Automatic fresh model review on verified dispatched runs when checks flag
//! something or the diff touches a risky path configured in `blyte.toml`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

enum ReviewReply {
    Ok(String),
    HttpError(u16, String),
}

struct MockServer {
    port: u16,
    review_calls: Arc<AtomicUsize>,
    review_requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl MockServer {
    fn start(bash_command: String, review_reply: ReviewReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let review_calls = Arc::new(AtomicUsize::new(0));
        let review_requests: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let review_calls_bg = Arc::clone(&review_calls);
        let review_requests_bg = Arc::clone(&review_requests);
        let implementer_turns = Arc::new(AtomicUsize::new(0));

        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let req_body = read_http_body(&mut stream);
                let req_json: serde_json::Value =
                    serde_json::from_str(&req_body).unwrap_or(serde_json::Value::Null);
                let is_review = req_json
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|first| first.get("content"))
                    .and_then(|c| c.as_str())
                    .is_some_and(|s| s.contains("performing a first review of a code diff"));

                if is_review {
                    review_calls_bg.fetch_add(1, Ordering::SeqCst);
                    review_requests_bg.lock().unwrap().push(req_json);
                    match &review_reply {
                        ReviewReply::Ok(text) => {
                            let body = serde_json::json!({
                                "id": "rev-1",
                                "object": "chat.completion",
                                "choices": [{
                                    "index": 0,
                                    "message": {
                                        "role": "assistant",
                                        "content": text,
                                    },
                                    "finish_reason": "stop",
                                }],
                                "usage": {
                                    "prompt_tokens": 50,
                                    "completion_tokens": 20,
                                    "total_tokens": 70,
                                }
                            })
                            .to_string();
                            write_http_response(&mut stream, 200, "OK", &body);
                        }
                        ReviewReply::HttpError(code, msg) => {
                            write_http_response(&mut stream, *code, "Error", msg);
                        }
                    }
                } else {
                    let turn = implementer_turns.fetch_add(1, Ordering::SeqCst);
                    let body = if turn == 0 {
                        let args = serde_json::json!({ "command": bash_command }).to_string();
                        serde_json::json!({
                            "id": "impl-1",
                            "object": "chat.completion",
                            "choices": [{
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "",
                                    "tool_calls": [{
                                        "id": "call_1",
                                        "type": "function",
                                        "function": {
                                            "name": "bash",
                                            "arguments": args,
                                        }
                                    }]
                                },
                                "finish_reason": "tool_calls",
                            }],
                            "usage": {
                                "prompt_tokens": 20,
                                "completion_tokens": 10,
                                "total_tokens": 30,
                            }
                        })
                        .to_string()
                    } else {
                        serde_json::json!({
                            "id": "impl-2",
                            "object": "chat.completion",
                            "choices": [{
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "done",
                                },
                                "finish_reason": "stop",
                            }],
                            "usage": {
                                "prompt_tokens": 20,
                                "completion_tokens": 5,
                                "total_tokens": 25,
                            }
                        })
                        .to_string()
                    };
                    write_http_response(&mut stream, 200, "OK", &body);
                }
            }
        });

        Self {
            port,
            review_calls,
            review_requests,
        }
    }
}

fn read_http_body(stream: &mut std::net::TcpStream) -> String {
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

fn write_http_response(stream: &mut std::net::TcpStream, status: u16, reason: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn git(dir: &Path, args: &[&str]) {
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
}

fn git_head(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(repo: &Path, blyte_toml: Option<&str>) {
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/app.rs"),
        "pub fn greet() -> &'static str { \"hi\" }\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/other.rs"),
        "pub fn helper() -> usize { 1 }\n",
    )
    .unwrap();
    if let Some(cfg) = blyte_toml {
        std::fs::write(repo.join("blyte.toml"), cfg).unwrap();
    }
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "t@t"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "init"]);
}

fn run_dispatch(
    repo: &Path,
    home: &Path,
    task_file: &Path,
    id: &str,
    port: u16,
) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            id,
            task_file.to_str().unwrap(),
            "--gate",
            "true",
            "--no-base-check",
            "--sandbox",
            "off",
        ])
        .current_dir(repo)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/gemini-3.8-flash")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "dispatch failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn collect_runs(repo: &Path, now: u64) -> blyte_cli::replay::Replay {
    let src = blyte_cli::dashboard::Sources {
        roots: vec![repo.to_path_buf()],
        runs_dirs: Vec::new(),
        hours: 24,
    };
    blyte_cli::replay::collect(&src, now, 1)
}

#[test]
fn risky_config_defaults_to_empty_and_matches_globs() {
    let d = tempfile::tempdir().unwrap();
    let cfg_path = d.path().join("blyte.toml");

    std::fs::write(&cfg_path, "[dispatch]\ngate = \"true\"\n").unwrap();
    let cfg = blyte_cli::config::load(d.path()).unwrap().unwrap();
    assert!(cfg.review.risky.is_empty());

    std::fs::write(
        &cfg_path,
        "[dispatch]\ngate = \"true\"\n\n[review]\nrisky = [\"migrations/**\", \"crates/auth/*.rs\"]\n",
    )
    .unwrap();
    let cfg = blyte_cli::config::load(d.path()).unwrap().unwrap();
    assert_eq!(cfg.review.risky, vec!["migrations/**", "crates/auth/*.rs"]);

    let changed = vec![
        "src/lib.rs".to_string(),
        "migrations/001_init.sql".to_string(),
        "crates/auth/token.rs".to_string(),
        "crates/auth/sub/deep.rs".to_string(),
    ];
    let matched = blyte_cli::config::match_risky_paths(&cfg.review.risky, &changed).unwrap();
    assert_eq!(
        matched,
        vec!["migrations/001_init.sql", "crates/auth/token.rs"]
    );

    std::fs::write(&cfg_path, "[review]\nrisky = [\"[unclosed\"]\n").unwrap();
    let err = blyte_cli::config::load(d.path()).unwrap_err();
    assert!(err.contains("[review] risky"), "{err}");
}

#[test]
fn should_run_model_review_policy() {
    assert!(!blyte_cli::review_checks::should_run_model_review(
        &[],
        Some("Update src/app.rs"),
        &[],
    ));

    assert!(!blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/app.rs".to_string()],
        Some("Improve the greeting text."),
        &[],
    ));

    assert!(blyte_cli::review_checks::should_run_model_review(
        &[
            "files not named in the task: src/app.rs".to_string(),
            "possible fallbacks added (1): src/app.rs:1".to_string(),
        ],
        Some("Improve the greeting text."),
        &[],
    ));

    assert!(!blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/other.rs".to_string()],
        Some("Update `src/app.rs`."),
        &[],
    ));

    assert!(blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/other.rs".to_string()],
        Some("## Files\n- `src/app.rs`\n\nUpdate `src/app.rs`."),
        &[],
    ));

    assert!(blyte_cli::review_checks::should_run_model_review(
        &[],
        Some("Update migrations/001.sql."),
        &["migrations/001.sql".to_string()],
    ));
}

#[test]
fn clean_run_starts_no_review() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs` so greet returns hello.\n").unwrap();

    let server = MockServer::start(
        "printf 'pub fn greet() -> &\\x27static str { \"hello\" }\\n' > src/app.rs && git add src/app.rs && git commit -m 'feat: greet hello'".to_string(),
        ReviewReply::Ok("[]".to_string()),
    );

    let summary = run_dispatch(&repo, &home, &task, "clean1", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 0);

    let checks_raw = std::fs::read_to_string(repo.join(".wt/clean1.checks.json")).unwrap();
    let checks: serde_json::Value = serde_json::from_str(&checks_raw).unwrap();
    assert_eq!(checks["flags"].as_array().unwrap().len(), 0, "{checks}");
    assert!(checks.get("model_review").is_none(), "{checks}");
    assert!(!repo.join(".wt/clean1.model-review.json").exists());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let replay = collect_runs(&repo, now);
    assert!(replay.skipped.is_empty(), "{:?}", replay.skipped);
    let run = replay.runs.iter().find(|r| r.id == "clean1").unwrap();
    assert_eq!(run.card, "clean");
}

#[test]
fn task_without_named_files_starts_no_review() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Change the greeting to hello.\n").unwrap();

    let server = MockServer::start(
        "printf 'pub fn greet() -> &\\x27static str { \"hello\" }\\n' > src/app.rs && git add src/app.rs && git commit -m 'feat: greet hello'".to_string(),
        ReviewReply::Ok("[]".to_string()),
    );

    let summary = run_dispatch(&repo, &home, &task, "nofiles1", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 0);

    let checks_raw = std::fs::read_to_string(repo.join(".wt/nofiles1.checks.json")).unwrap();
    let checks: serde_json::Value = serde_json::from_str(&checks_raw).unwrap();
    assert!(checks.get("model_review").is_none(), "{checks}");
    assert!(!repo.join(".wt/nofiles1.model-review.json").exists());
}

#[test]
fn flagged_run_triggers_fresh_review_and_findings_wait_in_queue() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);
    let head_before = git_head(&repo);

    let task = d.path().join("task.md");
    std::fs::write(
        &task,
        "## Files\n- `src/app.rs`\n\nUpdate `src/app.rs` so greet returns hello.\n",
    )
    .unwrap();

    let review_json = serde_json::json!([{
        "file": "src/other.rs",
        "line": 1,
        "severity": "high",
        "text": "modified src/other.rs instead of src/app.rs"
    }])
    .to_string();

    let server = MockServer::start(
        "printf 'pub fn helper() -> usize { 2 }\\n' > src/other.rs && git add src/other.rs && git commit -m 'feat: touch other'".to_string(),
        ReviewReply::Ok(review_json),
    );

    let summary = run_dispatch(&repo, &home, &task, "flagged1", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);

    let captured = server.review_requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let messages = captured[0]["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        2,
        "review must be a fresh run: {captured:?}"
    );
    let user_content = messages[1]["content"].as_str().unwrap();
    assert!(user_content.contains("## Task"), "{user_content}");
    assert!(user_content.contains("## Diff"), "{user_content}");
    assert!(user_content.contains("src/other.rs"), "{user_content}");

    let checks_raw = std::fs::read_to_string(repo.join(".wt/flagged1.checks.json")).unwrap();
    let checks: serde_json::Value = serde_json::from_str(&checks_raw).unwrap();
    assert_eq!(checks["model_review"]["status"].as_str(), Some("ok"));
    assert_eq!(
        checks["model_review"]["concerns"].as_array().unwrap().len(),
        1
    );
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags
            .iter()
            .any(|f| f.starts_with("model review (1 concern):")),
        "{flags:?}"
    );

    assert_eq!(git_head(&repo), head_before);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let replay = collect_runs(&repo, now);
    assert!(replay.skipped.is_empty(), "{:?}", replay.skipped);
    let run = replay.runs.iter().find(|r| r.id == "flagged1").unwrap();
    assert_eq!(run.card, "exception");
    assert!(run.verdict.is_none());

    blyte_cli::queue_actions::decide(
        &repo,
        "flagged1",
        blyte_cli::queue_actions::Action::Accept,
        None,
        now,
    )
    .unwrap();
    let replay_after = collect_runs(&repo, now);
    let run_after = replay_after
        .runs
        .iter()
        .find(|r| r.id == "flagged1")
        .unwrap();
    assert_eq!(run_after.verdict.as_deref(), Some("accepted"));
}

#[test]
fn risky_path_triggers_review_and_findings_put_run_in_queue() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(
        &repo,
        Some("[dispatch]\ngate = \"true\"\n\n[review]\nrisky = [\"migrations/**\"]\n"),
    );

    let task = d.path().join("task.md");
    std::fs::write(&task, "Add `migrations/001.sql` to create users table.\n").unwrap();

    let review_json = serde_json::json!([{
        "file": "migrations/001.sql",
        "line": 1,
        "severity": "high",
        "text": "missing NOT NULL constraint on id"
    }])
    .to_string();

    let server = MockServer::start(
        "mkdir -p migrations && printf 'CREATE TABLE users (id INT);\\n' > migrations/001.sql && git add migrations/001.sql && git commit -m 'feat: add migration'".to_string(),
        ReviewReply::Ok(review_json),
    );

    let summary = run_dispatch(&repo, &home, &task, "risky1", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);

    let checks_raw = std::fs::read_to_string(repo.join(".wt/risky1.checks.json")).unwrap();
    let checks: serde_json::Value = serde_json::from_str(&checks_raw).unwrap();
    assert_eq!(checks["model_review"]["status"].as_str(), Some("ok"));
    assert_eq!(
        checks["model_review"]["concerns"].as_array().unwrap().len(),
        1
    );
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert!(
        flags[0].starts_with("model review (1 concern):"),
        "{flags:?}"
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let replay = collect_runs(&repo, now);
    assert!(replay.skipped.is_empty(), "{:?}", replay.skipped);
    let run = replay.runs.iter().find(|r| r.id == "risky1").unwrap();
    assert_eq!(run.card, "exception");
    assert!(run.verdict.is_none());
}

#[test]
fn path_is_not_risky_unless_configured_in_blyte_toml() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, Some("[dispatch]\ngate = \"true\"\n"));

    let task = d.path().join("task.md");
    std::fs::write(&task, "Add `migrations/001.sql` to create users table.\n").unwrap();

    let server = MockServer::start(
        "mkdir -p migrations && printf 'CREATE TABLE users (id INT);\\n' > migrations/001.sql && git add migrations/001.sql && git commit -m 'feat: add migration'".to_string(),
        ReviewReply::Ok("[]".to_string()),
    );

    let summary = run_dispatch(&repo, &home, &task, "notrisky1", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 0);

    let checks_raw = std::fs::read_to_string(repo.join(".wt/notrisky1.checks.json")).unwrap();
    let checks: serde_json::Value = serde_json::from_str(&checks_raw).unwrap();
    assert!(checks.get("model_review").is_none(), "{checks}");
}

#[test]
fn failed_review_is_recorded_and_run_waits_in_queue() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(
        &repo,
        Some("[dispatch]\ngate = \"true\"\n\n[review]\nrisky = [\"migrations/**\"]\n"),
    );

    let task = d.path().join("task.md");
    std::fs::write(&task, "Add `migrations/001.sql` to create users table.\n").unwrap();

    let server = MockServer::start(
        "mkdir -p migrations && printf 'CREATE TABLE users (id INT);\\n' > migrations/001.sql && git add migrations/001.sql && git commit -m 'feat: add migration'".to_string(),
        ReviewReply::HttpError(400, "{\"error\":{\"message\":\"review backend error\"}}".to_string()),
    );

    let summary = run_dispatch(&repo, &home, &task, "revfail1", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);

    let checks_raw = std::fs::read_to_string(repo.join(".wt/revfail1.checks.json")).unwrap();
    let checks: serde_json::Value = serde_json::from_str(&checks_raw).unwrap();
    assert_eq!(checks["model_review"]["status"].as_str(), Some("failed"));
    assert!(
        checks["model_review"]["error"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{checks}"
    );
    assert_eq!(
        checks["model_review"]["concerns"].as_array().unwrap().len(),
        0
    );
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("model review failed:")),
        "{flags:?}"
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let replay = collect_runs(&repo, now);
    assert!(replay.skipped.is_empty(), "{:?}", replay.skipped);
    let run = replay.runs.iter().find(|r| r.id == "revfail1").unwrap();
    assert_eq!(run.card, "exception");
    assert!(run.verdict.is_none());
}
