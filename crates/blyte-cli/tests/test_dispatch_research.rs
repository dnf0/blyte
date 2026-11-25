//! Research is a dispatch: a scratch workspace under .wt/, a report gate, a `research` tag
//! (2026-10-01).

use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::{wrap_research, RESEARCH_RULES};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_STATE_DIR", dir.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn the_research_prompt_is_read_only_and_names_the_code() {
    let p = wrap_research("  How is the gate run?\n", Path::new("/repo/x"));
    assert!(p.starts_with("READ-ONLY RESEARCH TASK. The code to research is under /repo/x"));
    assert!(p.contains("How is the gate run?"));
    assert!(p.ends_with(RESEARCH_RULES));
    assert!(RESEARCH_RULES.contains("report.md"));
}

#[test]
fn a_research_dispatch_uses_a_scratch_repository_and_a_report_gate() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("q.md"), "Where is f.txt written?\n").unwrap();

    // No key: the run fails fast, after the workspace and start file exist.
    let out = blyte(
        r,
        &[
            "dispatch",
            "Q1",
            "q.md",
            "--research",
            "--no-brief-check",
            "--sandbox",
            "off",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(v["gate_source"], "research");
    assert!(v["research_report"].is_null());
    let scratch = r.join(".wt/Q1");
    assert!(
        scratch.join(".git").is_dir(),
        "a scratch repository of its own"
    );
    let wts = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(r)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&wts.stdout).lines().count(),
        1,
        "not a worktree"
    );
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/Q1.start.json")).unwrap())
            .unwrap();
    assert!(start["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "research"));
    assert_eq!(start["research"], true);
    assert_eq!(std::fs::read_to_string(r.join("f.txt")).unwrap(), "a\n");

    // The same id again is a clash; --research with --gate is a usage error.
    assert_eq!(
        blyte(
            r,
            &[
                "dispatch",
                "Q1",
                "q.md",
                "--research",
                "--no-brief-check",
                "--sandbox",
                "off"
            ]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        blyte(
            r,
            &["dispatch", "Q2", "q.md", "--research", "--gate", "true"]
        )
        .status
        .code(),
        Some(2)
    );
}

fn read_http_body(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;
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

fn start_report_writing_mock_server() -> u16 {
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let turn = AtomicUsize::new(0);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = read_http_body(&mut stream);
            let t = turn.fetch_add(1, Ordering::SeqCst);
            let body = if t == 0 {
                let args = serde_json::json!({
                    "command": "printf '# Findings\\n\\nReport body.\\n' > report.md"
                })
                .to_string();
                serde_json::json!({
                    "id": "r-tool",
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
                        "prompt_tokens": 100,
                        "completion_tokens": 50,
                        "total_tokens": 150,
                    }
                })
                .to_string()
            } else {
                serde_json::json!({
                    "id": "r-done",
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "wrote report.md",
                        },
                        "finish_reason": "stop",
                    }],
                    "usage": {
                        "prompt_tokens": 100,
                        "completion_tokens": 20,
                        "total_tokens": 120,
                    }
                })
                .to_string()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[test]
fn research_run_with_report_to_parent_as_file_errors_and_leaves_scratch_report() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().join("repo");
    let home = d.path().join("home");
    std::fs::create_dir_all(&r).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(&r, &["add", "."]);
    git(&r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("q.md"), "Where is f.txt written?\n").unwrap();

    let blocking_file = d.path().join("not_a_dir");
    std::fs::write(&blocking_file, "blocker").unwrap();
    let report_to = blocking_file.join("out.md");

    let port = start_report_writing_mock_server();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "RERR",
            "q.md",
            "--research",
            "--no-brief-check",
            "--sandbox",
            "off",
            "--report-to",
            report_to.to_str().unwrap(),
        ])
        .current_dir(&r)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
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
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"].as_str(), Some("error"), "{summary}");
    let err_msg = summary["error"].as_str().expect("error message");
    assert!(
        err_msg.contains(&report_to.display().to_string()),
        "error should name destination path: {err_msg}"
    );
    let scratch_report = r.join(".wt/RERR/report.md");
    assert!(
        scratch_report.exists(),
        "scratch report should stay in place"
    );
    assert!(
        std::fs::read_to_string(&scratch_report)
            .unwrap()
            .contains("Report body."),
        "scratch report content preserved"
    );
}
