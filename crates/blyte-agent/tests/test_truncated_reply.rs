use std::collections::VecDeque;
use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Event, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct ScriptedProvider {
    replies: Mutex<VecDeque<Response>>,
    requests: Mutex<Vec<Request>>,
}

impl ScriptedProvider {
    fn new(replies: Vec<Response>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.requests.lock().unwrap().push(req.clone());
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted provider ran out of replies"))
    }

    fn model_id(&self) -> String {
        "scripted".into()
    }
}

const HARNESS_NOTE: &str = "Your last reply hit the output token limit and was cut off, so none of it was used. Reply again more briefly: make one tool call at a time and keep edits small.";

#[test]
fn max_tokens_reply_skips_tool_call_and_adds_harness_note() {
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join("cutoff.txt");
    let provider = ScriptedProvider::new(vec![
        Response {
            text: "partial text".into(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "write".into(),
                arguments: json!({
                    "path": "cutoff.txt",
                    "content": "half written",
                }),
                signature: None,
            }],
            finish_reason: Some("MAX_TOKENS".into()),
            ..Default::default()
        },
        Response {
            text: "done now".into(),
            finish_reason: Some("STOP".into()),
            ..Default::default()
        },
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
    };

    let mut state = State::new("goal", "");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert!(
        !target.exists(),
        "truncated write tool call must not execute"
    );
    assert_eq!(report.truncated_turns, 1);
    assert_eq!(report.tool_calls, 0);
    assert_eq!(report.final_message.as_deref(), Some("done now"));

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|m| matches!(m, Message::User(s) if s == HARNESS_NOTE)),
        "expected second request to contain harness note, got {:?}",
        requests[1].messages
    );
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|m| matches!(m, Message::Assistant(_) | Message::ToolCall { .. })),
        "partial text and tool calls must not be in history: {:?}",
        requests[1].messages
    );
}

#[test]
fn continuation_reply_does_not_run_gate_until_normal_reply() {
    let d = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(vec![
        Response {
            text: "still going...".into(),
            finish_reason: Some("CONTINUATION".into()),
            ..Default::default()
        },
        Response {
            text: "finished".into(),
            finish_reason: Some("STOP".into()),
            ..Default::default()
        },
    ]);

    let events = Mutex::new(Vec::new());
    let on_event = |e: Event<'_>| match e {
        Event::Turn { n, .. } => events.lock().unwrap().push(format!("turn:{n}")),
        Event::GateStart { attempt } => {
            events.lock().unwrap().push(format!("gate_start:{attempt}"))
        }
        Event::Gate { attempt, .. } => events.lock().unwrap().push(format!("gate:{attempt}")),
        _ => {}
    };

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: Some(&on_event),
        research: false,
    };

    let mut state = State::new("goal", "true");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.truncated_turns, 1);
    assert_eq!(report.gate_attempts, 1);
    assert_eq!(report.gate_turns, vec![2]);
    assert_eq!(report.final_message.as_deref(), Some("finished"));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["turn:1", "turn:2", "gate_start:1", "gate:1"]
    );
}

#[test]
fn normal_stop_behaves_as_before() {
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join("written.txt");
    let provider = ScriptedProvider::new(vec![
        Response {
            text: "writing file".into(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "write".into(),
                arguments: json!({
                    "path": "written.txt",
                    "content": "hello",
                }),
                signature: None,
            }],
            finish_reason: Some("STOP".into()),
            ..Default::default()
        },
        Response {
            text: "all done".into(),
            finish_reason: Some("STOP".into()),
            ..Default::default()
        },
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
    };

    let mut state = State::new("goal", "true");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert_eq!(report.truncated_turns, 0);
    assert_eq!(report.tool_calls, 1);
    assert_eq!(report.gate_attempts, 1);
    assert_eq!(report.final_message.as_deref(), Some("all done"));
}

#[test]
fn other_provider_truncation_reasons_and_normal_stops() {
    for reason in ["max_tokens", "length", "incomplete"] {
        let d = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider::new(vec![
            Response {
                text: "cut off".into(),
                finish_reason: Some(reason.into()),
                ..Default::default()
            },
            Response {
                text: "ok".into(),
                finish_reason: Some("end_turn".into()),
                ..Default::default()
            },
        ]);
        let runner = Runner {
            provider: &provider,
            shell: &LocalShell,
            fs: &LocalFs,
            context: &Conversational,
            root: d.path().to_path_buf(),
            budget: Budget {
                max_turns: 5,
                max_gate_attempts: 1,
                max_duration: None,
            },
            rollback: false,
            on_event: None,
            research: false,
        };
        let mut state = State::new("goal", "");
        let (outcome, report) = runner.run(&mut state);
        assert_eq!(outcome, Outcome::Verified, "reason: {reason}");
        assert_eq!(report.truncated_turns, 1, "reason: {reason}");
        assert_eq!(report.final_message.as_deref(), Some("ok"));
    }

    for normal in [
        "STOP",
        "end_turn",
        "stop",
        "tool_use",
        "tool_calls",
        "completed",
    ] {
        let d = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider::new(vec![Response {
            text: "complete".into(),
            finish_reason: Some(normal.into()),
            ..Default::default()
        }]);
        let runner = Runner {
            provider: &provider,
            shell: &LocalShell,
            fs: &LocalFs,
            context: &Conversational,
            root: d.path().to_path_buf(),
            budget: Budget {
                max_turns: 5,
                max_gate_attempts: 1,
                max_duration: None,
            },
            rollback: false,
            on_event: None,
            research: false,
        };
        let mut state = State::new("goal", "");
        let (outcome, report) = runner.run(&mut state);
        assert_eq!(outcome, Outcome::Verified, "normal: {normal}");
        assert_eq!(report.truncated_turns, 0, "normal: {normal}");
    }
}
