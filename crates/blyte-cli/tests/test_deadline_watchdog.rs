//! A model call that never returns cannot keep a run past its deadline, and the run still leaves
//! a report: a dispatch sat 2 h 49 min under --max-seconds 3600 with none (2026-09-30).

use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn a_hung_model_call_ends_at_the_deadline_with_a_report() {
    // A server that accepts connections and never answers.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming().flatten() {
            held.push(s);
        }
    });

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

    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "say hello",
            "--gate",
            "true",
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
    let took = started.elapsed();

    assert!(took < Duration::from_secs(60), "ran {took:?}");
    assert!(
        took >= Duration::from_secs(30),
        "ended before the watchdog: {took:?}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("past --max-seconds 2"), "{err}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a report")).unwrap();
    assert!(
        v["error"].as_str().unwrap().contains("never returned"),
        "{v}"
    );
}
