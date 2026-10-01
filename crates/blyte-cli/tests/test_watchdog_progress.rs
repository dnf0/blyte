//! A run stopped at the deadline while its gate runs says so, with its turns and gate attempts
//! (2026-10-01).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

/// An OpenAI-compatible server whose model stops at once ("done"), so the gate runs.
fn model_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let mut s = s;
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            // Read headers and body (Content-Length) before answering.
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
            let body = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        }
    });
    port
}

#[test]
fn a_run_stopped_in_its_gate_reports_the_gate_and_its_progress() {
    let port = model_server();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("r");
    let home = d.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let report = d.path().join("report.json");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "say done",
            "--gate",
            "sleep 120",
            "--max-seconds",
            "2",
            "--sandbox",
            "off",
            "--report",
        ])
        .arg(&report)
        .current_dir(&repo)
        .env("HOME", &home)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/test-model")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    let err = v["error"].as_str().unwrap();
    assert!(err.contains("gate attempt 1 was still running"), "{err}");
    assert_eq!(v["turns"], 1, "{v}");
    assert_eq!(v["gate_attempts"], 1, "{v}");
}
