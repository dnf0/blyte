//! Each turn's `TurnUsage` carries the stream timing of its model call, and of every attempt
//! that failed and was retried.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, FailedAttempt, Outcome, Runner, TurnUsage};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, StreamTiming};

struct StubShell;

impl Shell for StubShell {
    fn exec(&self, command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        let exit_code = if command.lines().any(|l| l.trim() == "true") {
            0
        } else {
            1
        };
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code,
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

/// Each call answers with the next scripted result and reports the next scripted timing,
/// as a streaming provider does, including after a failed call.
struct Scripted {
    script: Mutex<Vec<(Result<Response, ProviderError>, StreamTiming)>>,
    last: Mutex<Option<StreamTiming>>,
}

impl Scripted {
    fn new(script: Vec<(Result<Response, ProviderError>, StreamTiming)>) -> Self {
        Self {
            script: Mutex::new(script),
            last: Mutex::new(None),
        }
    }
}

impl Provider for Scripted {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let (result, timing) = self.script.lock().unwrap().remove(0);
        *self.last.lock().unwrap() = Some(timing);
        result
    }
    fn model_id(&self) -> String {
        "scripted".into()
    }
    fn last_stream_timing(&self) -> Option<StreamTiming> {
        *self.last.lock().unwrap()
    }
}

/// A provider that does not override `last_stream_timing`.
struct NonStreaming;

impl Provider for NonStreaming {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "non-streaming".into()
    }
}

fn done() -> Result<Response, ProviderError> {
    Ok(Response {
        text: "done".into(),
        ..Default::default()
    })
}

fn run_with(provider: &dyn Provider) -> (Outcome, Vec<TurnUsage>) {
    let dir = tempfile::tempdir().unwrap();
    let runner = Runner {
        provider,
        shell: &StubShell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None::<Duration>,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    let mut state = State::new("g", "true");
    let (outcome, report) = runner.run_with_clock_and_sleep(&mut state, &Instant::now, &|_| {});
    (outcome, report.turn_usage)
}

#[test]
fn turn_usage_carries_the_calls_stream_timing() {
    let provider = Scripted::new(vec![(
        done(),
        StreamTiming {
            first_data_ms: Some(7),
            max_gap_ms: 9,
            data_events: 2,
        },
    )]);
    let (outcome, turns) = run_with(&provider);
    assert_eq!(outcome, Outcome::Verified);
    let first = &turns[0];
    assert_eq!(first.first_data_ms, Some(7));
    assert_eq!(first.max_gap_ms, 9);
    assert!(first.failed_attempts.is_empty());
}

#[test]
fn a_retried_timeout_is_recorded_as_a_failed_attempt() {
    let provider = Scripted::new(vec![
        (
            Err(ProviderError::Transport("timed out: read".into())),
            StreamTiming {
                first_data_ms: None,
                max_gap_ms: 300_000,
                data_events: 0,
            },
        ),
        (
            done(),
            StreamTiming {
                first_data_ms: Some(5),
                max_gap_ms: 6,
                data_events: 1,
            },
        ),
    ]);
    let (outcome, turns) = run_with(&provider);
    assert_eq!(outcome, Outcome::Verified);
    let first = &turns[0];
    assert_eq!(first.first_data_ms, Some(5));
    assert_eq!(first.max_gap_ms, 6);
    assert_eq!(first.failed_attempts.len(), 1);
    let failed: &FailedAttempt = &first.failed_attempts[0];
    assert!(failed.timed_out);
    assert_eq!(failed.first_data_ms, None);
    assert_eq!(failed.max_gap_ms, 300_000);
}

#[test]
fn a_provider_without_timing_records_none() {
    let (outcome, turns) = run_with(&NonStreaming);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(turns[0].first_data_ms, None);
    assert_eq!(turns[0].max_gap_ms, 0);
}

#[test]
fn old_reports_still_parse() {
    let old = serde_json::json!({
        "turn": 1,
        "input": 10,
        "cached": 0,
        "output": 2,
        "thinking": 0,
        "prompt_messages": 1,
        "appended": false,
        "new_chars": 4,
    });
    let turn: TurnUsage = serde_json::from_value(old).unwrap();
    assert_eq!(turn.first_data_ms, None);
    assert_eq!(turn.max_gap_ms, 0);
    assert!(turn.failed_attempts.is_empty());

    let json = serde_json::to_value(&turn).unwrap();
    assert!(json.get("failed_attempts").is_none());
}
