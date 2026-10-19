use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Event, Outcome, RunReport, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct DoneProvider;

impl Provider for DoneProvider {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        Ok(Response {
            text: "task complete".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "done".into()
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum RecordedGateEvent {
    GateStart { attempt: u32 },
    Gate { attempt: u32, passed: bool },
}

fn run_with_events(gate: &str, max_gates: u32) -> (Outcome, RunReport, Vec<RecordedGateEvent>) {
    let d = tempfile::tempdir().unwrap();
    let p = DoneProvider;
    let gate_events = Mutex::new(Vec::new());
    let on_event = |e: Event<'_>| match e {
        Event::GateStart { attempt } => {
            gate_events
                .lock()
                .unwrap()
                .push(RecordedGateEvent::GateStart { attempt });
        }
        Event::Gate {
            attempt, passed, ..
        } => {
            gate_events
                .lock()
                .unwrap()
                .push(RecordedGateEvent::Gate { attempt, passed });
        }
        _ => {}
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: max_gates,
            max_duration: None,
        },
        rollback: false,
        on_event: Some(&on_event),
        research: false,
        claim_check: false,
    };
    let mut state = State::new("goal", gate);
    let (outcome, report) = runner.run(&mut state);
    let events = gate_events.into_inner().unwrap();
    (outcome, report, events)
}

#[test]
fn empty_gate_is_skipped() {
    let (outcome, report, events) = run_with_events("", 1);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_attempts, 0);
    assert!(report.gate_turns.is_empty());
    assert!(report.gate_ms.is_empty());
    assert!(report.gate_wait_ms.is_empty());
    assert!(
        events.is_empty(),
        "expected no GateStart or Gate events, got {events:?}"
    );
}

#[test]
fn orchestrator_empty_gate_reports_finished() {
    let d = tempfile::tempdir().unwrap();
    let runner = Runner {
        provider: &DoneProvider,
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
        claim_check: false,
    };
    let mut state = State::new("orchestrate plan", "");
    state.orchestrator.instructions = Some("orchestrator prompt".into());
    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Finished);
    assert_eq!(report.gate_attempts, 0);
}

#[test]
fn whitespace_gate_is_skipped() {
    let (outcome, report, events) = run_with_events("  \n", 1);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_attempts, 0);
    assert!(report.gate_turns.is_empty());
    assert!(report.gate_ms.is_empty());
    assert!(report.gate_wait_ms.is_empty());
    assert!(
        events.is_empty(),
        "expected no GateStart or Gate events, got {events:?}"
    );
}

#[test]
fn non_empty_gate_runs_and_passes() {
    let (outcome, report, events) = run_with_events("true", 1);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_attempts, 1);
    assert_eq!(
        events,
        vec![
            RecordedGateEvent::GateStart { attempt: 1 },
            RecordedGateEvent::Gate {
                attempt: 1,
                passed: true,
            },
        ]
    );
}

#[test]
fn failing_gate_with_max_gates_one_is_not_verified() {
    let (outcome, report, events) = run_with_events("false", 1);
    assert_ne!(outcome, Outcome::Verified);
    assert_eq!(outcome, Outcome::GateFailed);
    assert_eq!(report.gate_attempts, 1);
    assert_eq!(
        events,
        vec![
            RecordedGateEvent::GateStart { attempt: 1 },
            RecordedGateEvent::Gate {
                attempt: 1,
                passed: false,
            },
        ]
    );
}
