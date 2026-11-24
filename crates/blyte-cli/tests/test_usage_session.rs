use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use blyte_agent::run::{Event, Outcome, RunReport};
use blyte_cli::task_runner::{run_task_with_state_and_events, TaskConfig};
use blyte_cli::usage_log::{read, session_from, Entry};

static ENV_LOCK: Mutex<()> = Mutex::new(());

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

fn scripted_openai_server(responses: Vec<String>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for body in responses {
            let Some(mut stream) = listener.incoming().flatten().next() else {
                break;
            };
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
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
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.flush();
        }
    });
    port
}

fn three_turn_responses() -> Vec<String> {
    vec![
        r#"{"id":"c1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"glob","arguments":"{\"pattern\":\"*.txt\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":120,"prompt_tokens_details":{"cached_tokens":20},"completion_tokens":35,"completion_tokens_details":{"reasoning_tokens":15},"total_tokens":155}}"#.to_string(),
        r#"{"id":"c2","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_2","type":"function","function":{"name":"glob","arguments":"{\"pattern\":\"*.txt\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":250,"prompt_tokens_details":{"cached_tokens":50},"completion_tokens":55,"completion_tokens_details":{"reasoning_tokens":25},"total_tokens":305}}"#.to_string(),
        r#"{"id":"c3","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":380,"prompt_tokens_details":{"cached_tokens":80},"completion_tokens":75,"completion_tokens_details":{"reasoning_tokens":35},"total_tokens":455}}"#.to_string(),
    ]
}

fn make_config(root: PathBuf, model: &str) -> TaskConfig {
    TaskConfig {
        prompt: "run three turns".to_string(),
        gate: None,
        context_strategy: "conversational".to_string(),
        model: Some(model.to_string()),
        container: None,
        max_turns: 5,
        max_gates: 1,
        rollback: false,
        root,
        container_workdir: "/workspace".into(),
        max_seconds: None,
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "off".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        orchestrate: false,
        dispatch_cmd: None,
    }
}

struct SharedThreeTurnRun {
    outcome: Outcome,
    report: RunReport,
    entries: Vec<Entry>,
    on_second_turn: Vec<Entry>,
}

fn shared_three_turn_run() -> &'static SharedThreeTurnRun {
    static RUN: OnceLock<SharedThreeTurnRun> = OnceLock::new();
    RUN.get_or_init(|| {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("usage.jsonl");
        let port = scripted_openai_server(three_turn_responses());
        let base_url = format!("http://127.0.0.1:{port}");
        let _env = EnvRestore::set(&[
            ("OPENAI_API_KEY", Some("test-key")),
            ("OPENAI_BASE_URL", Some(base_url.as_str())),
            ("BLYTE_USAGE_LOG", Some(log_path.to_str().unwrap())),
            ("BLYTE_SESSION", Some("sess-test-123")),
            ("CLAUDE_CODE_SESSION_ID", None),
            ("BLYTE_DISPATCH_ID", Some("disp-test-456")),
        ]);

        let config = make_config(dir.path().to_path_buf(), "openai/gemini-3.8-flash");
        let mut state = blyte_agent::state::State::new(&config.prompt, "");
        let on_second_turn: Mutex<Option<Vec<Entry>>> = Mutex::new(None);
        let (outcome, report) = run_task_with_state_and_events(&config, &mut state, &|event| {
            if let Event::Turn { n: 2, .. } = event {
                *on_second_turn.lock().unwrap() = Some(read(&log_path));
            }
        })
        .expect("run should succeed");
        let entries = read(&log_path);
        let snapshot = on_second_turn
            .into_inner()
            .unwrap()
            .expect("Turn 2 event should have fired");
        SharedThreeTurnRun {
            outcome,
            report,
            entries,
            on_second_turn: snapshot,
        }
    })
}

#[test]
fn test_session_from() {
    assert_eq!(
        session_from(Some("b".into()), Some("c".into())),
        Some("b".into())
    );
    assert_eq!(session_from(None, Some("c".into())), Some("c".into()));
    assert_eq!(
        session_from(Some("".into()), Some("c".into())),
        Some("c".into())
    );
    assert_eq!(session_from(Some("  ".into()), None), None);
    assert_eq!(session_from(None, None), None);
}

#[test]
fn test_session_from_whitespace_blyte_falls_back_to_claude() {
    assert_eq!(
        session_from(Some("  ".into()), Some("c".into())),
        Some("c".into())
    );
}

#[test]
fn each_turn_writes_a_line() {
    let run = shared_three_turn_run();
    assert_eq!(run.entries.len(), 4, "entries: {:?}", run.entries);
    for (i, entry) in run.entries[..3].iter().enumerate() {
        assert_eq!(entry.outcome, "turn", "entry {i} should be a turn line");
        assert_eq!(entry.turns, 1, "entry {i} should count 1 turn");
    }
    let closing = &run.entries[3];
    assert_eq!(closing.outcome, format!("{:?}", run.outcome).to_lowercase());
    assert_eq!(closing.turns, 0);
}

#[test]
fn lines_sum_to_the_run_total() {
    let run = shared_three_turn_run();
    let entries = &run.entries;
    assert_eq!(entries.len(), 4);

    let sum_turns: u32 = entries.iter().map(|e| e.turns).sum();
    let sum_input: u64 = entries.iter().map(|e| e.input).sum();
    let sum_cached: u64 = entries.iter().map(|e| e.cached).sum();
    let sum_output: u64 = entries.iter().map(|e| e.output).sum();
    let sum_thinking: u64 = entries.iter().map(|e| e.thinking).sum();
    let sum_cost: f64 = entries.iter().filter_map(|e| e.cost_usd).sum();

    assert_eq!(sum_turns, run.report.outcome_turns);
    assert_eq!(sum_input, run.report.usage.input);
    assert_eq!(sum_cached, run.report.usage.cached);
    assert_eq!(sum_output, run.report.usage.output);
    assert_eq!(sum_thinking, run.report.usage.thinking);

    let expected_cost = blyte_provider::pricing("openai/gemini-3.8-flash")
        .unwrap()
        .cost(&run.report.usage);
    assert!(
        (sum_cost - expected_cost).abs() < 1e-12,
        "sum_cost={sum_cost} expected={expected_cost}"
    );
}

#[test]
fn turn_lines_are_on_disk_before_the_run_ends() {
    let run = shared_three_turn_run();
    let snapshot = &run.on_second_turn;
    assert_eq!(
        snapshot.len(),
        2,
        "two turn lines should already be on disk on turn 2: {snapshot:?}"
    );
    assert!(snapshot.iter().all(|e| e.outcome == "turn" && e.turns == 1));
}

#[test]
fn turn_lines_carry_session_and_dispatch() {
    let run = shared_three_turn_run();
    assert_eq!(run.entries.len(), 4);
    for (i, entry) in run.entries.iter().enumerate() {
        assert_eq!(
            entry.session.as_deref(),
            Some("sess-test-123"),
            "entry {i} missing session"
        );
        assert_eq!(
            entry.dispatch.as_deref(),
            Some("disp-test-456"),
            "entry {i} missing dispatch"
        );
    }
}
