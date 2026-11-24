//! Fix round after a fresh review confirms fallbacks on a verified dispatch run.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};

enum ImplementerPass {
    RunBashThenDone {
        bash_command: String,
        final_message: String,
    },
    ReplyOnly {
        final_message: String,
    },
}

struct MockServer {
    port: u16,
    implementer_starts: Arc<AtomicUsize>,
    review_calls: Arc<AtomicUsize>,
    implementer_prompts: Arc<Mutex<Vec<String>>>,
}

impl MockServer {
    fn start(passes: Vec<ImplementerPass>, review_replies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let implementer_starts = Arc::new(AtomicUsize::new(0));
        let review_calls = Arc::new(AtomicUsize::new(0));
        let implementer_prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let starts_bg = Arc::clone(&implementer_starts);
        let reviews_bg = Arc::clone(&review_calls);
        let prompts_bg = Arc::clone(&implementer_prompts);
        let pass_queue: Arc<Mutex<VecDeque<ImplementerPass>>> =
            Arc::new(Mutex::new(passes.into_iter().collect()));
        let active_pass: Arc<Mutex<Option<(ImplementerPass, usize)>>> = Arc::new(Mutex::new(None));
        let review_queue: Arc<Mutex<VecDeque<String>>> =
            Arc::new(Mutex::new(review_replies.into_iter().collect()));

        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let req_body = read_http_body(&mut stream);
                let req_json: serde_json::Value =
                    serde_json::from_str(&req_body).unwrap_or(serde_json::Value::Null);
                let messages = req_json
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .cloned()
                    .unwrap_or_default();
                let is_review = messages
                    .first()
                    .and_then(|first| first.get("content"))
                    .and_then(|c| c.as_str())
                    .is_some_and(|s| s.contains("performing a first review of a code diff"));

                if is_review {
                    reviews_bg.fetch_add(1, Ordering::SeqCst);
                    let reply = review_queue
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_else(|| r#"{"concerns":[],"fallbacks":[]}"#.to_string());
                    let body = serde_json::json!({
                        "id": "rev",
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
                            "prompt_tokens": 50,
                            "completion_tokens": 20,
                            "total_tokens": 70,
                        }
                    })
                    .to_string();
                    write_http_response(&mut stream, 200, "OK", &body);
                } else {
                    let is_first_turn = messages.len() <= 2;
                    let mut active = active_pass.lock().unwrap();
                    if is_first_turn {
                        starts_bg.fetch_add(1, Ordering::SeqCst);
                        if let Some(user_msg) = messages
                            .get(1)
                            .and_then(|m| m.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            prompts_bg.lock().unwrap().push(user_msg.to_string());
                        }
                        let next = pass_queue.lock().unwrap().pop_front().unwrap_or(
                            ImplementerPass::ReplyOnly {
                                final_message: "done".to_string(),
                            },
                        );
                        *active = Some((next, 0));
                    }
                    let (pass, turn) = active.as_mut().unwrap();
                    let current_turn = *turn;
                    *turn += 1;

                    let body = match pass {
                        ImplementerPass::RunBashThenDone {
                            bash_command,
                            final_message,
                        } => {
                            if current_turn == 0 {
                                let args =
                                    serde_json::json!({ "command": bash_command }).to_string();
                                serde_json::json!({
                                    "id": "impl-tool",
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
                                        "prompt_tokens": 1000,
                                        "completion_tokens": 500,
                                        "total_tokens": 1500,
                                    }
                                })
                                .to_string()
                            } else {
                                serde_json::json!({
                                    "id": "impl-done",
                                    "object": "chat.completion",
                                    "choices": [{
                                        "index": 0,
                                        "message": {
                                            "role": "assistant",
                                            "content": final_message,
                                        },
                                        "finish_reason": "stop",
                                    }],
                                    "usage": {
                                        "prompt_tokens": 1000,
                                        "completion_tokens": 200,
                                        "total_tokens": 1200,
                                    }
                                })
                                .to_string()
                            }
                        }
                        ImplementerPass::ReplyOnly { final_message } => serde_json::json!({
                            "id": "impl-reply",
                            "object": "chat.completion",
                            "choices": [{
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": final_message,
                                },
                                "finish_reason": "stop",
                            }],
                            "usage": {
                                "prompt_tokens": 1000,
                                "completion_tokens": 200,
                                "total_tokens": 1200,
                            }
                        })
                        .to_string(),
                    };
                    write_http_response(&mut stream, 200, "OK", &body);
                }
            }
        });

        Self {
            port,
            implementer_starts,
            review_calls,
            implementer_prompts,
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
            git(repo, &["init", "-q"]);
            let config_path = repo.join(".git/config");
            let mut cfg_text = std::fs::read_to_string(&config_path).unwrap();
            cfg_text.push_str("\n[user]\n\temail = t@t\n\tname = t\n[commit]\n\tgpgsign = false\n");
            std::fs::write(&config_path, cfg_text).unwrap();
            git(repo, &["add", "."]);
            git(repo, &["commit", "-q", "-m", "init"]);
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

fn spawn_dispatch(repo: &Path, home: &Path, args: &[&str], port: u16) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(repo)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("BLYTE_STATE_DIR", home.join("state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_GATE_SLOT_DIR", home.join("slots"))
        .env("BLYTE_GATE_SLOTS", "16")
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
        .unwrap()
}

fn run_dispatch_ok(
    repo: &Path,
    home: &Path,
    task_file: &Path,
    id: &str,
    gate: &str,
    port: u16,
) -> serde_json::Value {
    let out = spawn_dispatch(
        repo,
        home,
        &[
            "dispatch",
            id,
            task_file.to_str().unwrap(),
            "--gate",
            gate,
            "--no-base-check",
            "--sandbox",
            "off",
        ],
        port,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "dispatch failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn confirmed_fallback_reply(file: &str, line: usize, reason: &str) -> String {
    serde_json::json!({
        "concerns": [],
        "fallbacks": [{
            "file": file,
            "line": line,
            "verdict": "fallback",
            "reason": reason
        }]
    })
    .to_string()
}

fn not_fallback_reply(file: &str, line: usize, reason: &str) -> String {
    serde_json::json!({
        "concerns": [],
        "fallbacks": [{
            "file": file,
            "line": line,
            "verdict": "not_fallback",
            "reason": reason
        }]
    })
    .to_string()
}

#[test]
fn unit_task_text_and_should_run_and_config() {
    let fallbacks = vec![
        blyte_cli::fix_round::ConfirmedFallback {
            file: "src/app.rs".to_string(),
            line: 12,
            text: "let region = cfg.region.unwrap_or(\"us\");".to_string(),
            reason: "Substitutes a hardcoded default region when missing.".to_string(),
        },
        blyte_cli::fix_round::ConfirmedFallback {
            file: "src/net.rs".to_string(),
            line: 44,
            text: "let timeout = raw.parse().unwrap_or(30);".to_string(),
            reason: "Silently replaces an unparseable timeout.".to_string(),
        },
    ];
    let original = "Add region and timeout support in `src/app.rs` and `src/net.rs`.";
    let text = blyte_cli::fix_round::task_text(original, &fallbacks);

    assert!(
        text.contains(blyte_cli::fix_round::COMMIT_MESSAGE),
        "{text}"
    );
    assert!(
        text.contains("src/app.rs:12 let region = cfg.region.unwrap_or(\"us\");"),
        "{text}"
    );
    assert!(
        text.contains("Substitutes a hardcoded default region when missing."),
        "{text}"
    );
    assert!(
        text.contains("src/net.rs:44 let timeout = raw.parse().unwrap_or(30);"),
        "{text}"
    );
    assert!(
        text.contains("Silently replaces an unparseable timeout."),
        "{text}"
    );
    let heading = "## Original task (for context; already done)";
    let heading_idx = text.find(heading).expect("missing original task heading");
    let orig_idx = text.find(original).expect("missing original task text");
    assert!(orig_idx > heading_idx, "{text}");

    let confirmed = blyte_cli::model_review::ModelReviewResult::ok(
        "gemini/gemini-3.8-flash".to_string(),
        Some(0.01),
        Vec::new(),
    )
    .with_fallbacks(vec![blyte_cli::model_review::FallbackVerdict {
        file: "src/app.rs".to_string(),
        line: 1,
        verdict: "fallback".to_string(),
        reason: "hides missing value".to_string(),
    }]);
    let not_fb = blyte_cli::model_review::ModelReviewResult::ok(
        "gemini/gemini-3.8-flash".to_string(),
        Some(0.01),
        Vec::new(),
    )
    .with_fallbacks(vec![blyte_cli::model_review::FallbackVerdict {
        file: "src/app.rs".to_string(),
        line: 1,
        verdict: "not_fallback".to_string(),
        reason: "prefix stripping".to_string(),
    }]);
    let empty_ok = blyte_cli::model_review::ModelReviewResult::ok(
        "gemini/gemini-3.8-flash".to_string(),
        Some(0.01),
        Vec::new(),
    );
    let failed = blyte_cli::model_review::ModelReviewResult::failed(
        Some("gemini/gemini-3.8-flash".to_string()),
        None,
        "timeout".to_string(),
    );

    assert!(blyte_cli::fix_round::should_run(
        false,
        "verified",
        true,
        Some(&confirmed)
    ));
    assert!(!blyte_cli::fix_round::should_run(
        true,
        "verified",
        true,
        Some(&confirmed)
    ));
    assert!(!blyte_cli::fix_round::should_run(
        false,
        "gate_failed",
        true,
        Some(&confirmed)
    ));
    assert!(!blyte_cli::fix_round::should_run(
        false,
        "verified",
        false,
        Some(&confirmed)
    ));
    assert!(!blyte_cli::fix_round::should_run(
        false, "verified", true, None
    ));
    assert!(!blyte_cli::fix_round::should_run(
        false,
        "verified",
        true,
        Some(&failed)
    ));
    assert!(!blyte_cli::fix_round::should_run(
        false,
        "verified",
        true,
        Some(&not_fb)
    ));
    assert!(!blyte_cli::fix_round::should_run(
        false,
        "verified",
        true,
        Some(&empty_ok)
    ));

    let d = tempfile::tempdir().unwrap();
    let cfg_path = d.path().join("blyte.toml");
    std::fs::write(&cfg_path, "[dispatch]\ngate = \"true\"\n").unwrap();
    let cfg = blyte_cli::config::load(d.path()).unwrap().unwrap();
    assert!(cfg.dispatch.fix_round);

    std::fs::write(
        &cfg_path,
        "[dispatch]\ngate = \"true\"\nfix_round = false\n",
    )
    .unwrap();
    let cfg = blyte_cli::config::load(d.path()).unwrap().unwrap();
    assert!(!cfg.dispatch.fix_round);

    std::fs::write(
        &cfg_path,
        "[dispatch]\ngate = \"true\"\nfix_round = \"yes\"\n",
    )
    .unwrap();
    let err = blyte_cli::config::load(d.path()).unwrap_err();
    assert!(err.contains("blyte.toml"), "{err}");

    let rendered = blyte_cli::config::render(&["true".to_string()]);
    assert!(rendered.contains("# fix_round = true"), "{rendered}");
}

#[test]
fn no_confirmed_fallback_runs_once_without_fix_round() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(
        vec![ImplementerPass::RunBashThenDone {
            bash_command: "printf 'pub fn f(s: &str) -> &str { s.strip_prefix(\"a\").unwrap_or(s) }\\n' > src/app.rs && git commit -am 'feat: strip'".to_string(),
            final_message: "pass 1 done".to_string(),
        }],
        vec![not_fallback_reply(
            "src/app.rs",
            1,
            "Keeping the original string on strip_prefix is normal.",
        )],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "NOFB", "true", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 1);
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);
    assert!(summary.get("fix_round").is_none(), "{summary}");
    assert!(!repo.join(".wt/NOFB.fixround.json").exists());
}

#[test]
fn fix_round_disabled_skips_round_and_keeps_fallback_flag() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(
        &repo,
        Some("[dispatch]\ngate = \"true\"\nfix_round = false\n"),
    );

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(
        vec![ImplementerPass::RunBashThenDone {
            bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: default region'".to_string(),
            final_message: "pass 1 done".to_string(),
        }],
        vec![confirmed_fallback_reply(
            "src/app.rs",
            1,
            "Substitutes hardcoded region.",
        )],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "DISABLED", "true", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 1);
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);
    assert!(summary.get("fix_round").is_none(), "{summary}");
    assert!(!repo.join(".wt/DISABLED.fixround.json").exists());

    let checks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/DISABLED.checks.json")).unwrap(),
    )
    .unwrap();
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("fallbacks added: 1")),
        "{flags:?}"
    );
}

#[test]
fn fix_round_kept_removes_fallback_and_sums_totals() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(
        &repo,
        Some("[dispatch]\ngate = \"true\"\n\n[review]\nrisky = [\"src/**\"]\n"),
    );

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs` to return the region.\n").unwrap();

    let server = MockServer::start(
        vec![
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: region'".to_string(),
                final_message: "pass 1 message".to_string(),
            },
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> Result<&str, &\\x27static str> { x.ok_or(\"missing region\") }\\n' > src/app.rs && git commit -am 'fix: fail loudly where review found a fallback'".to_string(),
                final_message: "fixed fallback".to_string(),
            },
        ],
        vec![
            confirmed_fallback_reply("src/app.rs", 1, "Substitutes hardcoded region."),
            serde_json::json!({"concerns": [], "fallbacks": []}).to_string(),
        ],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "KEPT1", "true", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 2);
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 2);

    let prompts = server.implementer_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("src/app.rs:1"), "{}", prompts[1]);
    assert!(
        prompts[1].contains("Substitutes hardcoded region."),
        "{}",
        prompts[1]
    );
    assert!(
        prompts[1].contains(blyte_cli::fix_round::COMMIT_MESSAGE),
        "{}",
        prompts[1]
    );
    assert!(
        prompts[1].contains("## Original task (for context; already done)"),
        "{}",
        prompts[1]
    );

    let fr = &summary["fix_round"];
    assert_eq!(fr["kept"].as_bool(), Some(true), "{summary}");
    assert_eq!(fr["outcome"].as_str(), Some("verified"), "{summary}");
    assert_eq!(fr["fallbacks_left"].as_u64(), Some(0), "{summary}");
    assert_eq!(fr["commits"].as_array().unwrap().len(), 1, "{summary}");
    assert_eq!(summary["commits"].as_array().unwrap().len(), 2, "{summary}");

    let pass1_rep: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/KEPT1.report.json")).unwrap())
            .unwrap();
    let fix_rep: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/KEPT1.fix.report.json")).unwrap(),
    )
    .unwrap();
    let expected_turns = pass1_rep["turns"].as_u64().unwrap() + fix_rep["turns"].as_u64().unwrap();
    assert_eq!(summary["turns"].as_u64(), Some(expected_turns), "{summary}");
    let expected_secs = pass1_rep["duration_seconds"].as_f64().unwrap().round() as u64
        + fix_rep["duration_seconds"].as_f64().unwrap().round() as u64;
    assert_eq!(
        summary["seconds"].as_u64(),
        Some(expected_secs),
        "{summary}"
    );
    if let (Some(c1), Some(c2)) = (pass1_rep["cost_usd"].as_f64(), fix_rep["cost_usd"].as_f64()) {
        let got = summary["cost_usd"].as_f64().unwrap();
        assert!((got - (c1 + c2)).abs() < 1e-9, "{got} vs {}", c1 + c2);
    }

    let final_msg = summary["final_message"].as_str().unwrap();
    assert!(final_msg.contains("pass 1 message"), "{final_msg}");
    assert!(final_msg.contains("Fix round:"), "{final_msg}");
    assert!(final_msg.contains("fixed fallback"), "{final_msg}");

    let checks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/KEPT1.checks.json")).unwrap())
            .unwrap();
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(!flags.iter().any(|f| f.contains("fallback")), "{flags:?}");

    let log_text = std::fs::read_to_string(repo.join(".wt/KEPT1.log")).unwrap();
    assert!(log_text.contains("== fix round"), "{log_text}");

    let fixround_rec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/KEPT1.fixround.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(fixround_rec["state"].as_str(), Some("done"));
    assert_eq!(fixround_rec["kept"].as_bool(), Some(true));
    assert_eq!(fixround_rec["fallbacks_left"].as_u64(), Some(0));
}

#[test]
fn fix_round_kept_with_one_still_confirmed_does_not_run_third_pass() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(
        vec![
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: region'".to_string(),
                final_message: "pass 1".to_string(),
            },
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"eu\") }\\n' > src/app.rs && git commit -am 'fix: fail loudly where review found a fallback'".to_string(),
                final_message: "still fallback".to_string(),
            },
        ],
        vec![
            confirmed_fallback_reply("src/app.rs", 1, "Substitutes us."),
            confirmed_fallback_reply("src/app.rs", 1, "Still substitutes eu."),
        ],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "STILL1", "true", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 2);
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 2);
    assert_eq!(summary["fix_round"]["kept"].as_bool(), Some(true));
    assert_eq!(summary["fix_round"]["fallbacks_left"].as_u64(), Some(1));

    let checks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/STILL1.checks.json")).unwrap(),
    )
    .unwrap();
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("fallbacks added: 1")),
        "{flags:?}"
    );
}

#[test]
fn fix_round_gate_failed_resets_worktree_and_appends_flag() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(
        vec![
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: region'".to_string(),
                final_message: "pass 1".to_string(),
            },
            ImplementerPass::RunBashThenDone {
                bash_command: "> FAIL_GATE && git add FAIL_GATE && git commit -m 'bad fix'".to_string(),
                final_message: "tried fix".to_string(),
            },
        ],
        vec![confirmed_fallback_reply(
            "src/app.rs",
            1,
            "Substitutes hardcoded region.",
        )],
    );

    let summary = run_dispatch_ok(
        &repo,
        &home,
        &task,
        "GFAIL",
        "test ! -e FAIL_GATE",
        server.port,
    );
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(summary["fix_round"]["kept"].as_bool(), Some(false));
    assert_eq!(
        summary["fix_round"]["outcome"].as_str(),
        Some("gate_failed")
    );
    assert_eq!(summary["fix_round"]["fallbacks_left"].as_u64(), Some(1));
    assert_eq!(summary["commits"].as_array().unwrap().len(), 1);

    let fixround_rec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/GFAIL.fixround.json")).unwrap(),
    )
    .unwrap();
    let pre_round_sha = fixround_rec["pre_round_sha"].as_str().unwrap();
    assert_eq!(git_head(&repo.join(".wt/GFAIL")), pre_round_sha);
    assert!(!repo.join(".wt/GFAIL/FAIL_GATE").exists());

    let checks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/GFAIL.checks.json")).unwrap())
            .unwrap();
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("fallbacks added: 1")),
        "{flags:?}"
    );
    assert!(
        flags.contains(&"fix round did not pass: gate_failed"),
        "{flags:?}"
    );
}

#[test]
fn fix_round_no_commit_marks_no_change_and_appends_flag() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(
        vec![
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: region'".to_string(),
                final_message: "pass 1".to_string(),
            },
            ImplementerPass::ReplyOnly {
                final_message: "leaving it as is".to_string(),
            },
        ],
        vec![confirmed_fallback_reply(
            "src/app.rs",
            1,
            "Substitutes hardcoded region.",
        )],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "NOCHG", "true", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(summary["fix_round"]["kept"].as_bool(), Some(false));
    assert_eq!(summary["fix_round"]["outcome"].as_str(), Some("no_change"));
    assert_eq!(summary["fix_round"]["fallbacks_left"].as_u64(), Some(1));
    assert_eq!(summary["commits"].as_array().unwrap().len(), 1);
    assert_eq!(summary["final_message"].as_str(), Some("pass 1"));

    let checks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/NOCHG.checks.json")).unwrap())
            .unwrap();
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("fallbacks added: 1")),
        "{flags:?}"
    );
    assert!(flags.contains(&"fix round made no change"), "{flags:?}");
}

#[test]
fn summary_does_not_exist_while_fix_round_is_running() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let summary_path = repo.join(".wt/NOSUM.json");
    let fixround_path = repo.join(".wt/NOSUM.fixround.json");
    let saw_fixround_marker = d.path().join("saw_fixround");
    let gate = format!(
        "test ! -e '{}' && (! [ -e '{}' ] || > '{}')",
        summary_path.display(),
        fixround_path.display(),
        saw_fixround_marker.display()
    );

    let server = MockServer::start(
        vec![
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: region'".to_string(),
                final_message: "pass 1".to_string(),
            },
            ImplementerPass::RunBashThenDone {
                bash_command: "printf 'pub fn f(x: Option<&str>) -> Result<&str, &\\x27static str> { x.ok_or(\"missing region\") }\\n' > src/app.rs && git commit -am 'fix: fail loudly where review found a fallback'".to_string(),
                final_message: "fixed".to_string(),
            },
        ],
        vec![
            confirmed_fallback_reply("src/app.rs", 1, "Substitutes hardcoded region."),
            serde_json::json!({"concerns": [], "fallbacks": []}).to_string(),
        ],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "NOSUM", &gate, server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(summary["fix_round"]["kept"].as_bool(), Some(true));
    assert!(saw_fixround_marker.exists());
    assert!(summary_path.exists());
}

#[test]
fn resume_started_fix_round_skips_first_pass_and_writes_summary() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(
        &repo,
        Some("[dispatch]\ngate = \"true\"\n\n[review]\nrisky = [\"src/**\"]\n"),
    );

    let base_sha = git_head(&repo);
    let wt_dir = repo.join(".wt/RES1");
    std::fs::create_dir_all(repo.join(".wt")).unwrap();
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wip/RES1",
            wt_dir.to_str().unwrap(),
        ],
    );

    std::fs::write(
        wt_dir.join("src/app.rs"),
        "pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\n",
    )
    .unwrap();
    git(&wt_dir, &["commit", "-q", "-am", "feat: region"]);
    let pre_round_sha = git_head(&wt_dir);

    std::fs::write(wt_dir.join("STRAY_FILE"), "should be reset\n").unwrap();
    git(&wt_dir, &["add", "STRAY_FILE"]);
    git(&wt_dir, &["commit", "-q", "-m", "stray interrupted commit"]);

    let task_rel = ".wt/RES1.task.md";
    std::fs::write(
        repo.join(task_rel),
        "Update `src/app.rs` to return the region.\n",
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/RES1.start.json"),
        serde_json::json!({
            "id": "RES1",
            "pid": 999999,
            "started_at": 1700000000u64,
            "task_file": task_rel,
            "base_sha": base_sha,
            "gate": "true",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/RES1.report.json"),
        serde_json::json!({
            "outcome": "verified",
            "turns": 3,
            "gate_attempts": 1,
            "gate_failures": [],
            "cost_usd": 0.05,
            "model_id": "openai/gemini-3.8-flash",
            "duration_seconds": 12.0,
            "sandbox": "off",
            "final_message": "first pass finished",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/RES1.checks.json"),
        serde_json::json!({
            "flags": ["fallbacks added: 1 (src/app.rs:1)"],
            "fallbacks": [{
                "file": "src/app.rs",
                "line": 1,
                "pattern": "unwrap_or",
                "text": "pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }",
                "verdict": "fallback",
                "reason": "Substitutes hardcoded region."
            }]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/RES1.fixround.json"),
        serde_json::json!({
            "state": "started",
            "pre_round_sha": pre_round_sha,
            "fallbacks_sent": [{
                "file": "src/app.rs",
                "line": 1,
                "text": "pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }",
                "reason": "Substitutes hardcoded region."
            }],
            "started_at": 1700000010u64,
        })
        .to_string(),
    )
    .unwrap();

    let server = MockServer::start(
        vec![ImplementerPass::RunBashThenDone {
            bash_command: "printf 'pub fn f(x: Option<&str>) -> Result<&str, &\\x27static str> { x.ok_or(\"missing region\") }\\n' > src/app.rs && git commit -am 'fix: fail loudly where review found a fallback'".to_string(),
            final_message: "resumed fix".to_string(),
        }],
        vec![serde_json::json!({"concerns": [], "fallbacks": []}).to_string()],
    );

    let out = spawn_dispatch(
        &repo,
        &home,
        &["dispatch", "RES1", "--no-base-check", "--sandbox", "off"],
        server.port,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "resume failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 1);
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);
    assert_eq!(summary["fix_round"]["kept"].as_bool(), Some(true));
    assert_eq!(summary["fix_round"]["fallbacks_left"].as_u64(), Some(0));
    assert_eq!(summary["commits"].as_array().unwrap().len(), 2);
    assert!(!wt_dir.join("STRAY_FILE").exists());
    assert!(repo.join(".wt/RES1.json").exists());

    let final_msg = summary["final_message"].as_str().unwrap();
    assert!(final_msg.contains("first pass finished"), "{final_msg}");
    assert!(final_msg.contains("Fix round:"), "{final_msg}");
    assert!(final_msg.contains("resumed fix"), "{final_msg}");

    let fixround_rec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".wt/RES1.fixround.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(fixround_rec["state"].as_str(), Some("done"));
}

#[test]
fn unreadable_fixround_json_errors_naming_file() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let wt_parent = repo.join(".wt");
    std::fs::create_dir_all(&wt_parent).unwrap();
    let bad_path = wt_parent.join("BAD1.fixround.json");
    std::fs::write(&bad_path, "{not valid json").unwrap();

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(Vec::new(), Vec::new());
    let out = spawn_dispatch(
        &repo,
        &home,
        &[
            "dispatch",
            "BAD1",
            task.to_str().unwrap(),
            "--gate",
            "true",
            "--no-base-check",
            "--sandbox",
            "off",
        ],
        server.port,
    );
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("BAD1.fixround.json"), "{stderr}");
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 0);
}

#[test]
fn done_fixround_never_runs_a_second_round() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    init_repo(&repo, None);

    let wt_parent = repo.join(".wt");
    std::fs::create_dir_all(&wt_parent).unwrap();
    let head = git_head(&repo);
    std::fs::write(
        wt_parent.join("DONE1.fixround.json"),
        serde_json::json!({
            "state": "done",
            "pre_round_sha": head,
            "fallbacks_sent": [],
            "started_at": 1700000000u64,
            "outcome": "verified",
            "turns": 1,
            "cost_usd": 0.01,
            "seconds": 2,
            "commits": [],
            "fallbacks_left": 1,
            "kept": true,
        })
        .to_string(),
    )
    .unwrap();

    let task = d.path().join("task.md");
    std::fs::write(&task, "Update `src/app.rs`.\n").unwrap();

    let server = MockServer::start(
        vec![ImplementerPass::RunBashThenDone {
            bash_command: "printf 'pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\\n' > src/app.rs && git commit -am 'feat: region'".to_string(),
            final_message: "pass 1".to_string(),
        }],
        vec![confirmed_fallback_reply(
            "src/app.rs",
            1,
            "Substitutes hardcoded region.",
        )],
    );

    let summary = run_dispatch_ok(&repo, &home, &task, "DONE1", "true", server.port);
    assert_eq!(summary["outcome"].as_str(), Some("verified"));
    assert_eq!(server.implementer_starts.load(Ordering::SeqCst), 1);
    assert_eq!(server.review_calls.load(Ordering::SeqCst), 1);
    assert!(summary.get("fix_round").is_none(), "{summary}");
}
