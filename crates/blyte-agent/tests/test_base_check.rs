use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{BaseCheck, BaseCheckHandle, BaseCheckState, Budget, Outcome, Runner};
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

fn make_runner<'a>(
    provider: &'a dyn Provider,
    root: &Path,
    base_check: BaseCheck<'a>,
) -> Runner<'a> {
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

    let (outcome, report) =
        make_runner(&provider, tmp.path(), BaseCheck::BeforeFirstTurn).run(&mut state);

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
    assert_eq!(report.base_check_passed, Some(false));
    assert!(report.base_check_seconds.is_some());
}

#[test]
fn base_check_on_passing_gate_calls_provider_and_preserves_gate_attempts() {
    let tmp_with = tempfile::tempdir().unwrap();
    let provider_with = Script::new(vec![Ok("echo hi > out.txt".into()), Err("Done.".into())]);
    let mut state_with = State::new("write out.txt", "true");
    let (outcome_with, report_with) =
        make_runner(&provider_with, tmp_with.path(), BaseCheck::BeforeFirstTurn)
            .run(&mut state_with);

    let tmp_without = tempfile::tempdir().unwrap();
    let provider_without = Script::new(vec![Ok("echo hi > out.txt".into()), Err("Done.".into())]);
    let mut state_without = State::new("write out.txt", "true");
    let (outcome_without, report_without) =
        make_runner(&provider_without, tmp_without.path(), BaseCheck::None).run(&mut state_without);

    assert_eq!(outcome_with, Outcome::Verified);
    assert_eq!(outcome_with, outcome_without);
    assert!(provider_with.call_count() > 0);
    assert_eq!(report_with.outcome_turns, report_without.outcome_turns);
    assert_eq!(report_with.gate_attempts, report_without.gate_attempts);
    assert_eq!(report_with.base_check_passed, Some(true));
    assert!(report_with.base_check_seconds.is_some());
    assert_eq!(report_without.base_check_passed, None);
    assert_eq!(report_without.base_check_seconds, None);
}

#[test]
fn base_check_off_failing_gate_calls_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("Done.".into())]);
    let mut state = State::new("do something", "false");

    let mut runner = make_runner(&provider, tmp.path(), BaseCheck::None);
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

    let (outcome, report) =
        make_runner(&provider, tmp.path(), BaseCheck::BeforeFirstTurn).run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(provider.call_count(), 1);
    assert_eq!(report.outcome_turns, 1);
}

struct ScriptedBaseCheck {
    polls: Mutex<Vec<BaseCheckState>>,
    wait_result: BaseCheckState,
    elapsed_on_poll: Option<f64>,
    elapsed_on_wait: Option<f64>,
    elapsed: Mutex<Option<f64>>,
    wait_calls: AtomicUsize,
    stopped: AtomicBool,
}

impl ScriptedBaseCheck {
    fn new(
        polls: Vec<BaseCheckState>,
        wait_result: BaseCheckState,
        elapsed_on_poll: Option<f64>,
        elapsed_on_wait: Option<f64>,
    ) -> Self {
        Self {
            polls: Mutex::new(polls),
            wait_result,
            elapsed_on_poll,
            elapsed_on_wait,
            elapsed: Mutex::new(None),
            wait_calls: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
        }
    }
}

impl BaseCheckHandle for ScriptedBaseCheck {
    fn poll(&self) -> BaseCheckState {
        let mut polls = self.polls.lock().unwrap();
        let state = if polls.len() > 1 {
            polls.remove(0)
        } else {
            polls.first().cloned().unwrap_or(BaseCheckState::Pending)
        };
        if !matches!(state, BaseCheckState::Pending) {
            *self.elapsed.lock().unwrap() = self.elapsed_on_poll;
        }
        state
    }

    fn wait(&self) -> BaseCheckState {
        self.wait_calls.fetch_add(1, Ordering::SeqCst);
        *self.elapsed.lock().unwrap() = self.elapsed_on_wait;
        self.wait_result.clone()
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    fn elapsed_seconds(&self) -> Option<f64> {
        *self.elapsed.lock().unwrap()
    }
}

#[test]
fn polled_passed_from_turn_1_runs_as_if_no_check() {
    let tmp_polled = tempfile::tempdir().unwrap();
    let provider_polled = Script::new(vec![Ok("echo hi > out.txt".into()), Err("Done.".into())]);
    let mut state_polled = State::new("write out.txt", "true");
    let handle = ScriptedBaseCheck::new(
        vec![BaseCheckState::Passed],
        BaseCheckState::Passed,
        Some(0.25),
        Some(0.25),
    );
    let (outcome_polled, report_polled) = make_runner(
        &provider_polled,
        tmp_polled.path(),
        BaseCheck::Polled(&handle),
    )
    .run(&mut state_polled);

    let tmp_none = tempfile::tempdir().unwrap();
    let provider_none = Script::new(vec![Ok("echo hi > out.txt".into()), Err("Done.".into())]);
    let mut state_none = State::new("write out.txt", "true");
    let (outcome_none, report_none) =
        make_runner(&provider_none, tmp_none.path(), BaseCheck::None).run(&mut state_none);

    assert_eq!(outcome_polled, Outcome::Verified);
    assert_eq!(outcome_polled, outcome_none);
    assert_eq!(provider_polled.call_count(), provider_none.call_count());
    assert_eq!(report_polled.outcome_turns, report_none.outcome_turns);
    assert_eq!(report_polled.gate_attempts, report_none.gate_attempts);
    assert_eq!(report_polled.final_message, report_none.final_message);
    assert_eq!(report_polled.base_check_passed, Some(true));
    assert_eq!(report_polled.base_check_seconds, Some(0.25));
    assert_eq!(handle.wait_calls.load(Ordering::SeqCst), 0);
    assert!(!handle.stopped.load(Ordering::SeqCst));
}

#[test]
fn polled_failed_at_turn_3_blocks_without_model_call_for_turn_3() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![
        Ok("echo 1".into()),
        Ok("echo 2".into()),
        Ok("echo 3".into()),
        Err("Done.".into()),
    ]);
    let mut state = State::new("do something", "true");
    let handle = ScriptedBaseCheck::new(
        vec![
            BaseCheckState::Pending,
            BaseCheckState::Pending,
            BaseCheckState::Failed {
                steps: vec!["cargo test".into()],
                tail: "assertion failed: 1 == 2".into(),
            },
        ],
        BaseCheckState::Pending,
        Some(1.5),
        None,
    );

    let (outcome, report) =
        make_runner(&provider, tmp.path(), BaseCheck::Polled(&handle)).run(&mut state);

    match outcome {
        Outcome::Blocked(ref msg) => {
            assert!(
                msg.contains("the check fails before any change"),
                "unexpected message: {msg}"
            );
            assert!(msg.contains("cargo test"), "missing step in message: {msg}");
            assert!(
                msg.contains("assertion failed: 1 == 2"),
                "missing tail in message: {msg}"
            );
            assert!(
                msg.contains("--no-base-check"),
                "missing flag hint in message: {msg}"
            );
        }
        other => panic!("expected Outcome::Blocked, got {other:?}"),
    }
    assert_eq!(provider.call_count(), 2);
    assert_eq!(report.outcome_turns, 2);
    assert_eq!(report.base_check_passed, Some(false));
    assert_eq!(report.base_check_seconds, Some(1.5));
}

#[test]
fn polled_pending_throughout_then_passed_at_final_ask_returns_verified() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("All done.".into())]);
    let mut state = State::new("do something", "true");
    let handle = ScriptedBaseCheck::new(
        vec![BaseCheckState::Pending],
        BaseCheckState::Passed,
        None,
        Some(2.0),
    );

    let (outcome, report) =
        make_runner(&provider, tmp.path(), BaseCheck::Polled(&handle)).run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(provider.call_count(), 1);
    assert_eq!(handle.wait_calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.base_check_passed, Some(true));
    assert_eq!(report.base_check_seconds, Some(2.0));
    assert_eq!(report.final_message.as_deref(), Some("All done."));
    assert!(!handle.stopped.load(Ordering::SeqCst));
}

#[test]
fn polled_pending_then_failed_at_final_ask_returns_verified_with_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("Fixed it.".into())]);
    let mut state = State::new("fix the failing check", "true");
    let handle = ScriptedBaseCheck::new(
        vec![BaseCheckState::Pending],
        BaseCheckState::Failed {
            steps: vec!["cargo test".into()],
            tail: "boom".into(),
        },
        None,
        Some(2.5),
    );

    let (outcome, report) =
        make_runner(&provider, tmp.path(), BaseCheck::Polled(&handle)).run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(provider.call_count(), 1);
    assert_eq!(handle.wait_calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.base_check_passed, Some(false));
    assert_eq!(report.base_check_seconds, Some(2.5));
    let msg = report.final_message.as_deref().unwrap_or("");
    assert!(msg.contains("Fixed it."), "got: {msg}");
    assert!(
        msg.contains("the check failed on the untouched code but passes with this change"),
        "got: {msg}"
    );
    assert!(!handle.stopped.load(Ordering::SeqCst));
}

#[test]
fn polled_pending_when_run_ends_other_way_stops_base_check() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![Err("Done.".into())]);
    let mut state = State::new("do something", "false");
    let handle = ScriptedBaseCheck::new(
        vec![BaseCheckState::Pending],
        BaseCheckState::Passed,
        None,
        None,
    );

    let mut runner = make_runner(&provider, tmp.path(), BaseCheck::Polled(&handle));
    runner.budget.max_gate_attempts = 1;
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::GateFailed);
    assert!(handle.stopped.load(Ordering::SeqCst));
    assert_eq!(handle.wait_calls.load(Ordering::SeqCst), 0);
    assert_eq!(report.base_check_passed, None);
    assert_eq!(report.base_check_seconds, None);
}
