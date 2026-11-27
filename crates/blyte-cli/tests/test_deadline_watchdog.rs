use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

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
        prompt: "say hi".to_string(),
        gate: None,
        context_strategy: "conversational".to_string(),
        model: Some("openai/test-model".to_string()),
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
        in_place: None,
    }
}

#[test]
fn hung_model_request_is_ended_by_max_seconds() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().unwrap().port();
    let connected = Arc::new(AtomicBool::new(false));
    let connected_srv = Arc::clone(&connected);
    let (done_tx, done_rx) = mpsc::channel::<()>();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            connected_srv.store(true, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = done_rx.recv_timeout(Duration::from_secs(60));
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.json");
    let usage_log = dir.path().join("usage.jsonl");
    let base_url = format!("http://127.0.0.1:{port}");
    let _env = EnvRestore::set(&[
        ("OPENAI_API_KEY", Some("test-key")),
        ("OPENAI_BASE_URL", Some(base_url.as_str())),
        ("BLYTE_USAGE_LOG", Some(usage_log.to_str().unwrap())),
        ("BLYTE_MODEL_TIMEOUT_SECS", Some("60")),
        ("BLYTE_MODEL_IDLE_TIMEOUT_SECS", Some("60")),
    ]);

    let config = make_config(dir.path().to_path_buf(), report.clone());
    let mut state = blyte_agent::state::State::new(&config.prompt, "");

    let started = Instant::now();
    let (outcome, _run_report) = run_task_with_state_and_max_duration(
        &config,
        &mut state,
        Some(Duration::from_millis(200)),
        &|_| {},
    )
    .expect("run should complete with BudgetExhausted");
    let took = started.elapsed();
    drop(done_tx);

    assert!(
        connected.load(Ordering::SeqCst),
        "blyte should have contacted the fake server"
    );
    assert!(
        took >= Duration::from_millis(150),
        "run ended before the deadline: took {took:?}"
    );
    assert_eq!(outcome, Outcome::BudgetExhausted);

    let raw = std::fs::read_to_string(&report).expect("report.json should be written");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid report JSON");
    assert_eq!(v["outcome"], "budget_exhausted", "report: {raw}");
    assert_eq!(v["turns"], 1, "report: {raw}");
    assert_eq!(
        v["turn_usage"].as_array().map(|a| a.len()),
        Some(1),
        "hung turn must appear in turn_usage: {raw}"
    );
}
