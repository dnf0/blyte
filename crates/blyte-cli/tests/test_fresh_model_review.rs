//! Automatic fresh model review on verified dispatched runs when checks flag
//! something or the diff touches a risky path configured in `blyte.toml`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
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

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

fn base_template_repo() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path();
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
            git(repo, &["init", "-q"]);
            let cfg_path = repo.join(".git/config");
            let mut cfg_text = std::fs::read_to_string(&cfg_path).unwrap();
            cfg_text.push_str("\n[user]\n\temail = t@t\n\tname = t\n[commit]\n\tgpgsign = false\n");
            std::fs::write(&cfg_path, cfg_text).unwrap();
            git(repo, &["add", "."]);
            git(repo, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
            dir
        })
        .path()
}

fn init_repo(repo: &Path, blyte_toml: Option<&str>) {
    copy_dir_all(base_template_repo(), repo);
    if let Some(cfg) = blyte_toml {
        std::fs::write(repo.join("blyte.toml"), cfg).unwrap();
    }
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
        .env("BLYTE_STATE_DIR", home.join("state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_GATE_SLOT_DIR", home.join("gate-slots"))
        .env("BLYTE_GATE_SLOTS", "16")
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_ENV_FILE", "")
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
        0,
    ));

    assert!(!blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/app.rs".to_string()],
        Some("Improve the greeting text."),
        &[],
        0,
    ));

    assert!(blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/app.rs".to_string()],
        Some("Improve the greeting text."),
        &[],
        1,
    ));

    assert!(!blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/other.rs".to_string()],
        Some("Update `src/app.rs`."),
        &[],
        0,
    ));

    assert!(blyte_cli::review_checks::should_run_model_review(
        &["files not named in the task: src/other.rs".to_string()],
        Some("## Files\n- `src/app.rs`\n\nUpdate `src/app.rs`."),
        &[],
        0,
    ));

    assert!(blyte_cli::review_checks::should_run_model_review(
        &[],
        Some("Update migrations/001.sql."),
        &["migrations/001.sql".to_string()],
        0,
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
        "printf 'pub fn greet() -> &\\x27static str { \"hello\" }\\n' > src/app.rs && git commit -am 'feat: greet hello'".to_string(),
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
        "printf 'pub fn greet() -> &\\x27static str { \"hello\" }\\n' > src/app.rs && git commit -am 'feat: greet hello'".to_string(),
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

    let review_json = serde_json::json!({
        "concerns": [{
            "file": "src/other.rs",
            "line": 1,
            "severity": "high",
            "text": "modified src/other.rs instead of src/app.rs"
        }],
        "fallbacks": []
    })
    .to_string();

    let server = MockServer::start(
        "printf 'pub fn helper() -> usize { 2 }\\n' > src/other.rs && git commit -am 'feat: touch other'".to_string(),
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

    let review_json = serde_json::json!({
        "concerns": [{
            "file": "migrations/001.sql",
            "line": 1,
            "severity": "high",
            "text": "missing NOT NULL constraint on id"
        }],
        "fallbacks": []
    })
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

struct SequenceProvider {
    model: String,
    responses: Mutex<Vec<Result<blyte_provider::Response, blyte_provider::ProviderError>>>,
    seen: Mutex<Vec<blyte_provider::Request>>,
}

impl SequenceProvider {
    fn new(
        model: &str,
        responses: Vec<Result<blyte_provider::Response, blyte_provider::ProviderError>>,
    ) -> Self {
        Self {
            model: model.to_string(),
            responses: Mutex::new(responses),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl blyte_provider::Provider for SequenceProvider {
    fn complete(
        &self,
        req: &blyte_provider::Request,
    ) -> Result<blyte_provider::Response, blyte_provider::ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            return Err(blyte_provider::ProviderError::Parse(
                "unexpected extra call".to_string(),
            ));
        }
        q.remove(0)
    }

    fn model_id(&self) -> String {
        self.model.clone()
    }
}

fn make_response(
    text: &str,
    finish_reason: Option<&str>,
    input: u64,
    output: u64,
) -> blyte_provider::Response {
    blyte_provider::Response {
        text: text.to_string(),
        tool_calls: Vec::new(),
        usage: blyte_provider::Usage {
            input,
            cached: 0,
            output,
            thinking: 0,
        },
        finish_reason: finish_reason.map(|s| s.to_string()),
    }
}

fn setup_worktree_run(
    root: &Path,
    id: &str,
    task: &str,
    files: &[(&str, &str)],
) -> (String, std::path::PathBuf) {
    let base = git_head(root);
    let wt_rel = format!(".wt/{id}");
    git(
        root,
        &["worktree", "add", "-q", "-b", &format!("wip/{id}"), &wt_rel],
    );
    std::fs::write(root.join("task.md"), task).unwrap();
    std::fs::write(
        root.join(format!(".wt/{id}.start.json")),
        serde_json::json!({
            "id": id,
            "base_sha": base,
            "task_file": "task.md",
        })
        .to_string(),
    )
    .unwrap();
    let wt = root.join(&wt_rel);
    let mut has_new = false;
    for (rel, body) in files {
        let p = wt.join(rel);
        if !p.exists() {
            has_new = true;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
    }
    if has_new {
        git(&wt, &["add", "."]);
        git(
            &wt,
            &["commit", "-q", "-m", "feat: change", "--no-gpg-sign"],
        );
    } else {
        git(
            &wt,
            &["commit", "-q", "-am", "feat: change", "--no-gpg-sign"],
        );
    }
    (base, wt)
}

#[test]
fn build_request_lists_candidates_only_when_present() {
    let without =
        blyte_cli::model_review::build_request("Update app", "stat", "+let x = 1;\n", &[]);
    let blyte_provider::Message::User(ref user_without) = without.messages[0] else {
        panic!("expected user message");
    };
    assert!(
        !user_without.contains("## Fallback candidates"),
        "{user_without}"
    );
    assert_eq!(without.max_output_tokens, Some(32_000));

    let candidates = vec![blyte_cli::fallback_check::Finding {
        file: "src/app.rs".to_string(),
        line: 12,
        pattern: "unwrap_or",
        text: "let region = cfg.region.unwrap_or(\"us\");".to_string(),
    }];
    let with = blyte_cli::model_review::build_request(
        "Update app",
        "stat",
        "+let region = cfg.region.unwrap_or(\"us\");\n",
        &candidates,
    );
    let blyte_provider::Message::User(ref user_with) = with.messages[0] else {
        panic!("expected user message");
    };
    assert!(user_with.contains("## Fallback candidates"), "{user_with}");
    assert!(user_with.contains("src/app.rs:12"), "{user_with}");
    assert!(user_with.contains("unwrap_or"), "{user_with}");
    assert!(
        user_with.contains("let region = cfg.region.unwrap_or(\"us\");"),
        "{user_with}"
    );
}

#[test]
fn parse_validates_object_format_fallbacks_key_and_verdict() {
    let ok = blyte_cli::model_review::parse(
        r#"{"concerns": [{"file": "src/a.rs", "line": 3, "severity": "high", "text": "bug"}], "fallbacks": [{"file": "src/a.rs", "line": 5, "verdict": "fallback", "reason": "hides missing config"}]}"#,
        true,
    )
    .unwrap();
    assert_eq!(ok.concerns.len(), 1);
    assert_eq!(ok.fallbacks.len(), 1);
    assert_eq!(ok.fallbacks[0].verdict, "fallback");

    let bare_err = blyte_cli::model_review::parse("[]", false).unwrap_err();
    assert!(bare_err.contains("JSON object"), "{bare_err}");

    let missing_fb = blyte_cli::model_review::parse(r#"{"concerns": []}"#, true).unwrap_err();
    assert!(missing_fb.contains("fallbacks"), "{missing_fb}");

    let bad_verdict = blyte_cli::model_review::parse(
        r#"{"concerns": [], "fallbacks": [{"file": "src/a.rs", "line": 1, "verdict": "maybe", "reason": "unsure"}]}"#,
        true,
    )
    .unwrap_err();
    assert!(bad_verdict.contains("verdict"), "{bad_verdict}");
}

#[test]
fn fallback_flags_reflect_review_verdicts_and_errors() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    init_repo(&repo, None);

    let (_base1, _wt1) = setup_worktree_run(
        &repo,
        "FB_OK",
        "Update `src/app.rs`.\n",
        &[(
            "src/app.rs",
            "pub fn f(s: &str) -> &str { s.strip_prefix(\"a\").unwrap_or(s) }\n",
        )],
    );
    let ok_reply = serde_json::json!({
        "concerns": [],
        "fallbacks": [{
            "file": "src/app.rs",
            "line": 1,
            "verdict": "not_fallback",
            "reason": "String prefix stripping keeps the original string when no prefix matches."
        }]
    })
    .to_string();
    blyte_cli::queue_actions::evaluate_verified_run_with(&repo, "FB_OK", |root, id, base, wt| {
        let prov = SequenceProvider::new(
            "gemini/gemini-3.8-flash",
            vec![Ok(make_response(&ok_reply, Some("STOP"), 100, 50))],
        );
        blyte_cli::model_review::run_with_provider(root, id, base, wt, &prov)
    })
    .unwrap();
    let checks1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/FB_OK.checks.json")).unwrap())
            .unwrap();
    assert!(checks1["flags"].as_array().unwrap().is_empty(), "{checks1}");
    assert_eq!(
        checks1["fallbacks"][0]["verdict"].as_str(),
        Some("not_fallback")
    );
    assert_eq!(
        checks1["fallbacks"][0]["reason"].as_str(),
        Some("String prefix stripping keeps the original string when no prefix matches.")
    );
    assert_eq!(
        checks1["fallbacks"][0]["text"].as_str(),
        Some("pub fn f(s: &str) -> &str { s.strip_prefix(\"a\").unwrap_or(s) }")
    );

    let (_base2, _wt2) = setup_worktree_run(
        &repo,
        "FB_CONFIRMED",
        "Update `src/app.rs`.\n",
        &[(
            "src/app.rs",
            "pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\n",
        )],
    );
    let confirmed_reply = serde_json::json!({
        "concerns": [],
        "fallbacks": [{
            "file": "src/app.rs",
            "line": 1,
            "verdict": "fallback",
            "reason": "Substitutes a hardcoded region when missing."
        }]
    })
    .to_string();
    blyte_cli::queue_actions::evaluate_verified_run_with(
        &repo,
        "FB_CONFIRMED",
        |root, id, base, wt| {
            let prov = SequenceProvider::new(
                "gemini/gemini-3.8-flash",
                vec![Ok(make_response(&confirmed_reply, Some("STOP"), 100, 50))],
            );
            blyte_cli::model_review::run_with_provider(root, id, base, wt, &prov)
        },
    )
    .unwrap();
    let checks2: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/FB_CONFIRMED.checks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        checks2["flags"],
        serde_json::json!(["fallbacks added: 1 (src/app.rs:1)"])
    );

    let _ = std::fs::remove_file(repo.join(".wt/FB_CONFIRMED.checks.json"));
    let _ = std::fs::remove_file(repo.join(".wt/FB_CONFIRMED.model-review.json"));
    let missing_reply = serde_json::json!({
        "concerns": [],
        "fallbacks": []
    })
    .to_string();
    blyte_cli::queue_actions::evaluate_verified_run_with(
        &repo,
        "FB_CONFIRMED",
        |root, id, base, wt| {
            let prov = SequenceProvider::new(
                "gemini/gemini-3.8-flash",
                vec![Ok(make_response(&missing_reply, Some("STOP"), 100, 50))],
            );
            blyte_cli::model_review::run_with_provider(root, id, base, wt, &prov)
        },
    )
    .unwrap();
    let checks3: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/FB_CONFIRMED.checks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        checks3["flags"],
        serde_json::json!(["fallback candidates not judged: 1 (src/app.rs:1)"])
    );

    let _ = std::fs::remove_file(repo.join(".wt/FB_CONFIRMED.checks.json"));
    let _ = std::fs::remove_file(repo.join(".wt/FB_CONFIRMED.model-review.json"));
    blyte_cli::queue_actions::evaluate_verified_run_with(
        &repo,
        "FB_CONFIRMED",
        |root, id, base, wt| {
            let prov = SequenceProvider::new(
                "gemini/gemini-3.8-flash",
                vec![Err(blyte_provider::ProviderError::Api {
                    status: 500,
                    body: "boom".to_string(),
                })],
            );
            blyte_cli::model_review::run_with_provider(root, id, base, wt, &prov)
        },
    )
    .unwrap();
    let checks4: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/FB_CONFIRMED.checks.json")).unwrap(),
    )
    .unwrap();
    let flags4: Vec<&str> = checks4["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags4
            .iter()
            .any(|f| f.starts_with("possible fallbacks added: 1 (not judged: ")),
        "{flags4:?}"
    );

    let mut long_findings = Vec::new();
    let mut long_verdicts = Vec::new();
    for i in 1..=20 {
        let file = format!("crates/very-long-package-name/src/module_{i:02}.rs");
        long_findings.push(blyte_cli::fallback_check::Finding {
            file: file.clone(),
            line: i,
            pattern: "unwrap_or",
            text: "x.unwrap_or(0)".to_string(),
        });
        long_verdicts.push(blyte_cli::model_review::FallbackVerdict {
            file,
            line: i,
            verdict: "fallback".to_string(),
            reason: "hides missing value".to_string(),
        });
    }
    let long_review = blyte_cli::model_review::ModelReviewResult::ok(
        "gemini/gemini-3.8-flash".to_string(),
        Some(0.0),
        Vec::new(),
    )
    .with_fallbacks(long_verdicts);
    let long_flags =
        blyte_cli::fallback_check::flags_for_review(&long_findings, Some(&long_review));
    assert_eq!(long_flags.len(), 1);
    assert!(long_flags[0].chars().count() <= 200, "{}", long_flags[0]);
    assert!(
        long_flags[0].starts_with("fallbacks added: 20 ("),
        "{}",
        long_flags[0]
    );
    assert!(long_flags[0].contains(", … and "), "{}", long_flags[0]);
    assert!(long_flags[0].ends_with(" more)"), "{}", long_flags[0]);
}

#[test]
fn cutoff_review_retries_once_and_sums_cost() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    init_repo(&repo, None);

    let (base1, wt1) = setup_worktree_run(
        &repo,
        "CUT1",
        "Update `src/app.rs`.\n",
        &[("src/app.rs", "pub fn greet() -> &'static str { \"hi2\" }\n")],
    );
    let valid_reply = serde_json::json!({
        "concerns": [{
            "file": "src/app.rs",
            "line": 1,
            "severity": "medium",
            "text": "check greeting"
        }],
        "fallbacks": []
    })
    .to_string();
    let prov1 = SequenceProvider::new(
        "gemini/gemini-3.8-flash",
        vec![
            Ok(make_response("", Some("MAX_TOKENS"), 100_000, 100_000)),
            Ok(make_response(&valid_reply, Some("STOP"), 100_000, 50_000)),
        ],
    );
    let res1 = blyte_cli::model_review::run_with_provider(&repo, "CUT1", &base1, &wt1, &prov1);
    assert_eq!(prov1.calls(), 2);
    {
        let seen = prov1.seen.lock().unwrap();
        assert_eq!(seen[0].max_output_tokens, Some(32_000));
        assert_eq!(seen[1].max_output_tokens, Some(64_000));
    }
    assert_eq!(res1.status, "ok");
    assert_eq!(res1.concerns.len(), 1);
    let pricing = blyte_provider::pricing("gemini/gemini-3.8-flash").unwrap();
    let expected_cost = pricing.cost(&blyte_provider::Usage {
        input: 100_000,
        cached: 0,
        output: 100_000,
        thinking: 0,
    }) + pricing.cost(&blyte_provider::Usage {
        input: 100_000,
        cached: 0,
        output: 50_000,
        thinking: 0,
    });
    assert!((res1.cost_usd.unwrap() - expected_cost).abs() < 1e-9);

    std::fs::copy(
        repo.join(".wt/CUT1.start.json"),
        repo.join(".wt/CUT2.start.json"),
    )
    .unwrap();
    let prov2 = SequenceProvider::new(
        "gemini/gemini-3.8-flash",
        vec![
            Ok(make_response("", Some("MAX_TOKENS"), 1000, 1000)),
            Ok(make_response("", Some("MAX_TOKENS"), 1000, 1000)),
        ],
    );
    let res2 = blyte_cli::model_review::run_with_provider(&repo, "CUT2", &base1, &wt1, &prov2);
    assert_eq!(prov2.calls(), 2);
    assert_eq!(res2.status, "failed");
    let err2 = res2.error.as_deref().unwrap_or("");
    assert!(err2.contains("(after one retry)"), "{err2}");

    std::fs::copy(
        repo.join(".wt/CUT1.start.json"),
        repo.join(".wt/CUT3.start.json"),
    )
    .unwrap();
    let prov3 = SequenceProvider::new(
        "gemini/gemini-3.8-flash",
        vec![Ok(make_response(&valid_reply, Some("STOP"), 1000, 500))],
    );
    let res3 = blyte_cli::model_review::run_with_provider(&repo, "CUT3", &base1, &wt1, &prov3);
    assert_eq!(prov3.calls(), 1);
    assert_eq!(res3.status, "ok");
}

#[test]
fn old_model_review_json_without_fallbacks_still_loads() {
    let d = tempfile::tempdir().unwrap();
    let wt_dir = d.path().join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    let old_path = wt_dir.join("OLD1.model-review.json");
    std::fs::write(
        &old_path,
        serde_json::json!({
            "model": "gemini/gemini-3.8-flash",
            "ts": 1700000000u64,
            "cost_usd": 0.01,
            "concerns": [{"file": "src/a.rs", "line": 2, "severity": "low", "text": "note"}],
            "error": null
        })
        .to_string(),
    )
    .unwrap();
    let loaded = blyte_cli::model_review::load_saved_review(&old_path).unwrap();
    assert_eq!(loaded.model.as_deref(), Some("gemini/gemini-3.8-flash"));
    assert_eq!(loaded.concerns.len(), 1);
    assert!(loaded.fallbacks.is_empty());
}
