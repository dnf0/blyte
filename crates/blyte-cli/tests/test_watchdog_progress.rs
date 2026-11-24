use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::run::Outcome;
use blyte_cli::task_runner::{run_task_with_state_and_max_duration, TaskConfig};

struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvRestore {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let mut prev = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            prev.push((*k, std::env::var_os(k)));
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        Self(prev)
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, v) in self.0.drain(..) {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

fn make_config(root: PathBuf, report: PathBuf) -> TaskConfig {
    TaskConfig {
        prompt: "finish immediately".to_string(),
        gate: Some("sleep 120".to_string()),
        context_strategy: "conversational".to_string(),
        model: Some("openai/gemini-3.8-flash".to_string()),
        container: None,
        max_turns: 5,
        max_gates: 1,
        rollback: false,
        root,
        container_workdir: "/workspace".into(),
        max_seconds: Some(1),
        max_cost_usd: None,
        report: Some(report),
        fold_budget: None,
        sandbox: "off".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
    }
}

#[test]
fn gate_deadline_records_budget_exhausted_and_progress() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().unwrap().port();

    let response_body = r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let content_len = text[..header_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= header_end + 4 + content_len {
                        break;
                    }
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.flush();
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let slot_dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.json");
    let usage_log = dir.path().join("usage.jsonl");
    let base_url = format!("http://127.0.0.1:{port}");
    let _env = EnvRestore::set(&[
        ("OPENAI_API_KEY", Some("test-key")),
        ("OPENAI_BASE_URL", Some(base_url.as_str())),
        (
            "BLYTE_GATE_SLOT_DIR",
            Some(slot_dir.path().to_str().unwrap()),
        ),
        ("BLYTE_USAGE_LOG", Some(usage_log.to_str().unwrap())),
    ]);

    let config = make_config(dir.path().to_path_buf(), report.clone());
    let mut state = blyte_agent::state::State::new(&config.prompt, "sleep 120");
    let (outcome, _run_report) = run_task_with_state_and_max_duration(
        &config,
        &mut state,
        Some(Duration::from_millis(100)),
        &|_| {},
    )
    .expect("run should complete with BudgetExhausted");

    assert_eq!(outcome, Outcome::BudgetExhausted);

    let raw = std::fs::read_to_string(&report).expect("report.json should be written");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid report JSON");

    assert_eq!(v["outcome"], "budget_exhausted", "report: {raw}");
    let gate_failures = v["gate_failures"].as_array().expect("gate_failures array");
    assert!(
        gate_failures
            .first()
            .and_then(|s| s.as_str())
            .is_some_and(|s| s.contains("gate ran out of time")),
        "gate_failures should note deadline expiry, got: {raw}"
    );
    assert_eq!(v["turns"], 1, "report: {raw}");
    assert_eq!(v["gate_attempts"], 1, "report: {raw}");
    assert_eq!(v["usage"]["input"], 10, "report: {raw}");
    assert_eq!(v["usage"]["cached"], 0, "report: {raw}");
    assert_eq!(v["usage"]["output"], 2, "report: {raw}");
    assert_eq!(v["usage"]["thinking"], 0, "report: {raw}");

    let expected_cost = blyte_provider::pricing("openai/gemini-3.8-flash")
        .expect("pricing known")
        .cost(&blyte_provider::Usage {
            input: 10,
            cached: 0,
            output: 2,
            thinking: 0,
        });
    let actual_cost = v["cost_usd"].as_f64().expect("cost_usd should be recorded");
    assert!(
        (actual_cost - expected_cost).abs() < 1e-12,
        "expected cost {expected_cost}, got {actual_cost}"
    );
}
