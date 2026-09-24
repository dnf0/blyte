//! Resilience: server retry hints, a cost cap, and checkpoints a run can be resumed from.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use superstate_agent::context::Conversational;
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{
    retry_delay, retry_wait, server_retry_hint, Budget, Event, Outcome, Runner,
};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{
    pricing, Message, Provider, ProviderError, Request, Response, ToolCall, Usage,
};

struct Scripted {
    model: String,
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Scripted {
    fn new(model: &str, script: Vec<Response>) -> Self {
        Self {
            model: model.into(),
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
        Ok(if s.is_empty() {
            Response::default()
        } else {
            s.remove(0)
        })
    }
    fn model_id(&self) -> String {
        self.model.clone()
    }
}

fn bash(id: &str, command: &str, usage: Usage) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: "bash".into(),
            arguments: json!({ "command": command }),
            signature: None,
        }],
        usage,
        ..Default::default()
    }
}

fn budget(max_turns: u32, max_cost_usd: Option<f64>) -> Budget {
    Budget {
        max_turns,
        max_gate_attempts: 1,
        max_duration: None,
        max_cost_usd,
    }
}

fn api(status: u16, body: &str) -> ProviderError {
    ProviderError::Api {
        status,
        body: body.into(),
    }
}

#[test]
fn the_server_retry_hint_is_honoured_and_bounded() {
    let body = r#"{"error": {"code": 429, "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "37s"}]}}"#;
    assert_eq!(
        server_retry_hint(&api(429, body)),
        Some(Duration::from_secs(37))
    );
    assert_eq!(retry_wait(&api(429, body), 1), Duration::from_secs(37));
    // A hint shorter than the backoff does not shorten it.
    let short = r#"{"retryDelay": "1s"}"#;
    assert_eq!(retry_wait(&api(429, short), 3), retry_delay(3));
    // No hint: the backoff.
    assert_eq!(retry_wait(&api(503, "overloaded"), 2), retry_delay(2));
    assert_eq!(
        server_retry_hint(&ProviderError::Transport("reset".into())),
        None
    );
    // An absurd hint is capped.
    let long = r#"{"retryDelay": "900s"}"#;
    assert_eq!(
        server_retry_hint(&api(429, long)),
        Some(Duration::from_secs(120))
    );
}

#[test]
fn gemini_flash_is_priced_with_thinking_billed_as_output() {
    let p = pricing("gemini/gemini-3.8-flash").expect("a known price");
    let u = Usage {
        input: 1_000_000,
        cached: 1_000_000,
        output: 100_000,
        thinking: 100_000,
    };
    assert!((p.cost(&u) - 1.575).abs() < 1e-9, "{}", p.cost(&u));
    assert_eq!(pricing("scripted"), None);
}

#[test]
fn a_run_stops_itself_at_its_cost_cap() {
    let d = tempfile::tempdir().unwrap();
    let million = Usage {
        input: 1_000_000,
        ..Default::default()
    };
    // $0.75 per turn; the cap of $2 is reached after three turns.
    let script = (0..6)
        .map(|i| bash(&format!("c{i}"), "echo hi", million))
        .collect();
    let p = Scripted::new("gemini/gemini-3.8-flash", script);
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(10, Some(2.0)),
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (outcome, report) = runner.run(&mut State::new("loop", "true"));
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(p.seen.lock().unwrap().len(), 3);
    assert_eq!(report.usage.input, 3_000_000);
}

#[test]
fn a_cost_cap_without_a_known_price_is_an_error_not_ignored() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted::new("scripted", vec![]);
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(10, Some(1.0)),
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (outcome, _) = runner.run(&mut State::new("x", "true"));
    assert!(
        matches!(&outcome, Outcome::Error(e) if e.contains("needs a known price")),
        "{outcome:?}"
    );
    assert!(p.seen.lock().unwrap().is_empty());
}

#[test]
fn a_run_resumes_from_its_last_checkpoint_without_rerunning_tools() {
    let d = tempfile::tempdir().unwrap();
    let saved: Mutex<Option<(Vec<Message>, State)>> = Mutex::new(None);
    let on_event = |e: Event<'_>| {
        if let Event::Checkpoint { history, state } = e {
            *saved.lock().unwrap() = Some((history.to_vec(), state.clone()));
        }
    };

    // First run: one turn that appends to a log, then the turn budget runs out.
    let p1 = Scripted::new(
        "scripted",
        vec![bash(
            "c0",
            "echo ran >> log.txt && echo logged",
            Usage::default(),
        )],
    );
    let r1 = Runner {
        provider: &p1,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(1, None),
        rollback: false,
        loop_breaker: false,
        on_event: Some(&on_event),
    };
    let mut st = State::new("log once", "test -f log.txt");
    let (outcome, _) = r1.run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    let (history, state) = saved.lock().unwrap().take().expect("a checkpoint");
    assert!(
        history.iter().any(
            |m| matches!(m, Message::ToolResult { content, .. } if content.contains("logged"))
        ),
        "the final checkpoint includes the last turn's results: {history:?}"
    );

    // Resume: the model sees the saved history, and nothing already run is run again.
    let p2 = Scripted::new(
        "scripted",
        vec![Response {
            text: "done".into(),
            ..Default::default()
        }],
    );
    let r2 = Runner {
        provider: &p2,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: budget(5, None),
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let mut st2 = state;
    let (outcome, _) = r2.resume(&mut st2, history.clone());
    assert_eq!(outcome, Outcome::Verified);
    let first = p2.seen.lock().unwrap()[0].clone();
    assert_eq!(&first.messages[..history.len()], &history[..]);
    let log = std::fs::read_to_string(d.path().join("log.txt")).unwrap();
    assert_eq!(log.lines().count(), 1, "the tool ran exactly once: {log:?}");
}
