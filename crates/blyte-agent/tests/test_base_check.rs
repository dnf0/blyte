use std::path::Path;
use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct Script {
    steps: Mutex<Vec<Result<String, String>>>,
    calls: Mutex<usize>,
}

impl Script {
    fn new(steps: Vec<Result<String, String>>) -> Self {
        Self {
            steps: Mutex::new(steps),
            calls: Mutex::new(0),
        }
    }

    fn call_count(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl Provider for Script {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        let mut s = self.steps.lock().unwrap();
        let next = if s.is_empty() {
            Err("Nothing more to do.".to_string())
        } else {
            s.remove(0)
        };
        Ok(match next {
            Ok(cmd) => Response {
                tool_calls: vec![ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: json!({"command": cmd}),
                    signature: None,
                }],
                ..Default::default()
            },
            Err(text) => Response {
                text,
                ..Default::default()
            },
        })
    }
    fn model_id(&self) -> String {
        "gemini/gemini-3.8-flash".into()
    }
}

fn make_runner<'a>(provider: &'a dyn Provider, root: &Path, base_check: bool) -> Runner<'a> {
    Runner {
        provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 3,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check,
    }
}

#[test]
fn base_check_on_failing_gate_returns_blocked_with_zero_provider_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("Done.".into())]);
    let mut state = State::new("do something", "false");

    let (outcome, report) = make_runner(&provider, tmp.path(), true).run(&mut state);

    match outcome {
        Outcome::Blocked(ref msg) => {
            assert!(
                msg.contains("the check fails before any change"),
                "unexpected message: {msg}"
            );
            assert!(
                msg.contains("false"),
                "expected failing step 'false' in message: {msg}"
            );
            assert!(
                msg.contains("--no-base-check"),
                "expected --no-base-check hint in message: {msg}"
            );
        }
        other => panic!("expected Outcome::Blocked, got {other:?}"),
    }
    assert_eq!(provider.call_count(), 0);
    assert_eq!(report.outcome_turns, 0);
    assert_eq!(report.gate_attempts, 0);
}

#[test]
fn base_check_on_passing_gate_calls_provider_and_preserves_gate_attempts() {
    let tmp_with = tempfile::tempdir().unwrap();
    let provider_with = Script::new(vec![Ok("echo hi > out.txt".into()), Err("Done.".into())]);
    let mut state_with = State::new("write out.txt", "true");
    let (outcome_with, report_with) =
        make_runner(&provider_with, tmp_with.path(), true).run(&mut state_with);

    let tmp_without = tempfile::tempdir().unwrap();
    let provider_without = Script::new(vec![Ok("echo hi > out.txt".into()), Err("Done.".into())]);
    let mut state_without = State::new("write out.txt", "true");
    let (outcome_without, report_without) =
        make_runner(&provider_without, tmp_without.path(), false).run(&mut state_without);

    assert_eq!(outcome_with, Outcome::Verified);
    assert_eq!(outcome_with, outcome_without);
    assert!(provider_with.call_count() > 0);
    assert_eq!(report_with.outcome_turns, report_without.outcome_turns);
    assert_eq!(report_with.gate_attempts, report_without.gate_attempts);
}

#[test]
fn base_check_off_failing_gate_calls_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("Done.".into())]);
    let mut state = State::new("do something", "false");

    let mut runner = make_runner(&provider, tmp.path(), false);
    runner.budget.max_gate_attempts = 1;
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::GateFailed);
    assert!(provider.call_count() > 0);
    assert_eq!(report.outcome_turns, 1);
}

#[test]
fn base_check_on_empty_gate_calls_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("Done.".into())]);
    let mut state = State::new("do something", "");

    let (outcome, report) = make_runner(&provider, tmp.path(), true).run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(provider.call_count(), 1);
    assert_eq!(report.outcome_turns, 1);
}
