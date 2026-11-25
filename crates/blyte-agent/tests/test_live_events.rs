use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Event, Outcome, Runner, MAX_PROVIDER_RETRIES};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

struct StubShell;

impl Shell for StubShell {
    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn exec_host(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.exec(command, cwd, timeout)
    }

    fn describe(&self) -> String {
        String::new()
    }

    fn available_binaries(
        &self,
        _cwd: &Path,
        _candidates: &[&'static str],
    ) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

struct ScriptedProvider {
    steps: Mutex<Vec<Result<Response, ProviderError>>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Result<Response, ProviderError>>) -> Self {
        Self {
            steps: Mutex::new(steps),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut steps = self.steps.lock().unwrap();
        if steps.is_empty() {
            Ok(Response {
                text: "done".into(),
                ..Default::default()
            })
        } else {
            steps.remove(0)
        }
    }

    fn model_id(&self) -> String {
        "scripted-live-events".into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordedEvent {
    Turn(u32),
    ToolStart {
        name: String,
        input: String,
    },
    Tool {
        name: String,
        input: String,
    },
    Retry {
        attempt: u32,
        max: u32,
        wait_ms: u64,
        reason: String,
    },
    Other,
}

fn record(event: Event<'_>) -> RecordedEvent {
    match event {
        Event::Turn { n, .. } => RecordedEvent::Turn(n),
        Event::ToolStart { name, input } => RecordedEvent::ToolStart {
            name: name.to_string(),
            input,
        },
        Event::Tool { name, input, .. } => RecordedEvent::Tool {
            name: name.to_string(),
            input,
        },
        Event::Retry {
            attempt,
            max,
            wait_ms,
            reason,
        } => RecordedEvent::Retry {
            attempt,
            max,
            wait_ms,
            reason,
        },
        Event::GateStart { .. }
        | Event::GateSlotWait { .. }
        | Event::Gate { .. }
        | Event::RolledBack { .. } => RecordedEvent::Other,
    }
}

fn run_with_events(
    provider: &ScriptedProvider,
    root: &std::path::Path,
) -> (Outcome, blyte_agent::run::RunReport, Vec<RecordedEvent>) {
    let events = Mutex::new(Vec::new());
    let on_event = |e: Event<'_>| {
        events.lock().unwrap().push(record(e));
    };
    let runner = Runner {
        provider,
        shell: &StubShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: Some(&on_event),
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    let mut state = State::new("live events test", "");
    let (outcome, report) = runner.run(&mut state);
    let collected = events.into_inner().unwrap();
    (outcome, report, collected)
}

#[test]
fn tool_start_precedes_each_tool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello\nworld\n").unwrap();

    let provider = ScriptedProvider::new(vec![
        Ok(Response {
            text: "running two tools".into(),
            tool_calls: vec![
                ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "echo alpha"}),
                    signature: None,
                },
                ToolCall {
                    id: "c2".into(),
                    name: "read".into(),
                    arguments: json!({"path": "note.txt", "start_line": 1, "end_line": 2}),
                    signature: None,
                },
            ],
            usage: Usage::default(),
            finish_reason: None,
        }),
        Ok(Response {
            text: "all done".into(),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: None,
        }),
    ]);

    let (outcome, _report, events) = run_with_events(&provider, dir.path());
    assert_eq!(outcome, Outcome::Verified);

    let tool_events: Vec<&RecordedEvent> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                RecordedEvent::ToolStart { .. } | RecordedEvent::Tool { .. }
            )
        })
        .collect();
    assert_eq!(tool_events.len(), 4, "events: {events:?}");

    match (tool_events[0], tool_events[1]) {
        (
            RecordedEvent::ToolStart {
                name: n1,
                input: i1,
            },
            RecordedEvent::Tool {
                name: n2,
                input: i2,
            },
        ) => {
            assert_eq!(n1, "bash");
            assert_eq!(n1, n2);
            assert_eq!(i1, i2);
        }
        other => panic!("expected ToolStart then Tool for first call, got {other:?}"),
    }

    match (tool_events[2], tool_events[3]) {
        (
            RecordedEvent::ToolStart {
                name: n1,
                input: i1,
            },
            RecordedEvent::Tool {
                name: n2,
                input: i2,
            },
        ) => {
            assert_eq!(n1, "read");
            assert_eq!(n1, n2);
            assert_eq!(i1, i2);
        }
        other => panic!("expected ToolStart then Tool for second call, got {other:?}"),
    }
}

#[test]
fn retry_event_is_emitted_before_the_retry() {
    std::env::set_var("BLYTE_RETRY_DELAY_MS", "1");
    let dir = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(vec![
        Err(ProviderError::Api {
            status: 429,
            body: "rate limited".into(),
        }),
        Ok(Response {
            text: "recovered".into(),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: None,
        }),
    ]);

    let (outcome, report, events) = run_with_events(&provider, dir.path());
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.provider_retries, 1);

    let retry_positions: Vec<(usize, &RecordedEvent)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, RecordedEvent::Retry { .. }))
        .collect();
    assert_eq!(retry_positions.len(), 1, "events: {events:?}");

    let (retry_idx, retry_event) = retry_positions[0];
    match retry_event {
        RecordedEvent::Retry {
            attempt,
            max,
            wait_ms,
            reason,
        } => {
            assert_eq!(*attempt, 1);
            assert_eq!(*max, MAX_PROVIDER_RETRIES);
            assert_eq!(*wait_ms, 1);
            assert!(!reason.is_empty());
        }
        _ => unreachable!(),
    }

    let first_turn_idx = events
        .iter()
        .position(|e| matches!(e, RecordedEvent::Turn(_)))
        .expect("expected at least one Turn event");
    assert!(
        retry_idx < first_turn_idx,
        "expected Retry ({retry_idx}) before first Turn ({first_turn_idx}); events: {events:?}"
    );
}

#[test]
fn no_retry_event_without_a_retry() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(vec![Ok(Response {
        text: "clean finish".into(),
        tool_calls: vec![],
        usage: Usage::default(),
        finish_reason: None,
    })]);

    let (outcome, report, events) = run_with_events(&provider, dir.path());
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.provider_retries, 0);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RecordedEvent::Retry { .. })),
        "unexpected Retry event in {events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, RecordedEvent::Turn(1))));
}
