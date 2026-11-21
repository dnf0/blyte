//! A model call that never returns cannot keep a run past its deadline, and the run still leaves
//! a report: a dispatch sat 2 h 49 min under --max-seconds 3600 with none (2026-09-30).

use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn a_hung_model_call_ends_at_the_deadline_with_a_report() {
    // A server that accepts connections and never answers.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let connected = Arc::new(AtomicBool::new(false));
    let connected_srv = Arc::clone(&connected);
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming().flatten() {
            connected_srv.store(true, Ordering::SeqCst);
            held.push(s);
        }
    });

    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("r");
    let home = d.path().join("home");
    let slots = d.path().join("slots");
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
        .env("XDG_CONFIG_HOME", &home)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_WATCHDOG_GRACE_SECS")
        .env_remove("BLYTE_MODEL_TIMEOUT_SECS")
        .env_remove("BLYTE_IN_GATE")
        .env("BLYTE_GATE_SLOT_DIR", &slots)
        .env("BLYTE_GATE_SLOTS", "2")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/test-model")
        .output()
        .unwrap();
    let took = started.elapsed();

    assert!(
        connected.load(Ordering::SeqCst),
        "expected blyte to connect to the hung server"
    );
    assert!(
        took >= Duration::from_secs(1),
        "ended before the deadline: {took:?}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Task finished with outcome: BudgetExhausted"),
        "{err}"
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a report")).unwrap();
    assert_eq!(v["outcome"].as_str(), Some("budget_exhausted"), "{v}");
    assert_eq!(v["turns"], 1, "{v}");
    assert_eq!(v["turn_usage"].as_array().map(|a| a.len()), Some(1), "{v}");
}
