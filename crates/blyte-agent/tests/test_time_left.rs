//! Time-limited runs are warned once when under 10% of `max_duration` remains; the turn-based and
//! time-based warnings share a single `wrap_up_sent` guard so at most one fires per run.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct FastShell;

impl Shell for FastShell {
    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn describe(&self) -> String {
        String::new()
    }
}

struct Busy {
    requests: Mutex<Vec<Vec<Message>>>,
    start: Instant,
    step_at: Option<(usize, Duration)>,
    end_at: Option<(usize, Duration)>,
}

impl Provider for Busy {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let mut reqs = self.requests.lock().unwrap();
        reqs.push(req.messages.clone());
        let call = reqs.len();
        drop(reqs);

        if let Some((target_call, target_elapsed)) = self.step_at {
            if call == target_call {
                if let Some(rem) = target_elapsed.checked_sub(self.start.elapsed()) {
                    std::thread::sleep(rem);
                }
            }
        }
        if let Some((target_call, target_elapsed)) = self.end_at {
            if call >= target_call {
                if let Some(rem) = target_elapsed.checked_sub(self.start.elapsed()) {
                    std::thread::sleep(rem);
                }
            }
        }

        Ok(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{call}"),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
                signature: None,
            }],
            ..Default::default()
        })
    }

    fn model_id(&self) -> String {
        "gemini/gemini-3.8-flash".into()
    }
}

fn count_user_messages_with(messages: &[Message], needle: &str) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, Message::User(t) if t.contains(needle)))
        .count()
}

fn ended_with_user_message(messages: &[Message], needle: &str) -> bool {
    matches!(messages.last(), Some(Message::User(t)) if t.contains(needle))
}

#[test]
fn time_warning_arrives_once_under_ten_percent_of_max_duration() {
    const BUDGET: Duration = Duration::from_secs(5);
    let d = tempfile::tempdir().unwrap();
    let p = Busy {
        requests: Mutex::new(Vec::new()),
        start: Instant::now(),
        // Step into the final 10% window (4.5 s..5.0 s) on call 2, and past the deadline on call 4,
        // while FastShell avoids spawning OS processes per turn.
        step_at: Some((2, Duration::from_millis(4550))),
        end_at: Some((4, BUDGET)),
    };
    let r = Runner {
        provider: &p,
        shell: &FastShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 300,
            max_gate_attempts: 1,
            max_duration: Some(BUDGET),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };
    let mut st = State::new("g", "true");
    let (outcome, _) = r.run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);

    let time_warn = "You have about 1 minutes left of 1. Wrap up now: make sure your work is saved before you run out: committed, or written to the file the task asks for. Then stop calling tools.";
    let reqs = p.requests.lock().unwrap();
    assert!(
        !reqs.is_empty(),
        "provider must have been called before the deadline"
    );
    let newly_warned = reqs
        .iter()
        .filter(|msgs| ended_with_user_message(msgs, time_warn))
        .count();
    assert_eq!(
        newly_warned, 1,
        "the time warning must be pushed at the top of a turn in exactly one request"
    );
    assert_eq!(
        count_user_messages_with(reqs.last().unwrap(), time_warn),
        1,
        "the time warning must appear once in history"
    );
    for msgs in reqs.iter() {
        assert_eq!(
            count_user_messages_with(msgs, "You have 10 turns left"),
            0,
            "the turn-based warning must never appear"
        );
    }
}

#[test]
fn without_time_limit_only_turn_warning_appears_at_turn_21() {
    let d = tempfile::tempdir().unwrap();
    let p = Busy {
        requests: Mutex::new(Vec::new()),
        start: Instant::now(),
        step_at: None,
        end_at: None,
    };
    let r = Runner {
        provider: &p,
        shell: &FastShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 30,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut st = State::new("g", "true");
    let (outcome, _) = r.run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);

    let turn_warn = "You have 10 turns left of 30. Wrap up now: make sure your work is saved before you run out: committed, or written to the file the task asks for. Then stop calling tools.";
    let reqs = p.requests.lock().unwrap();
    assert_eq!(reqs.len(), 30);
    let first = reqs
        .iter()
        .position(|msgs| count_user_messages_with(msgs, turn_warn) > 0)
        .expect("the turn warning was sent");
    assert_eq!(first, 20, "the 21st call is the first with ten turns left");
    assert_eq!(
        count_user_messages_with(reqs.last().unwrap(), turn_warn),
        1,
        "the turn warning must appear only once in history"
    );
    for msgs in reqs.iter() {
        assert_eq!(
            count_user_messages_with(msgs, "minutes left of"),
            0,
            "the time warning must never appear"
        );
    }
}

#[test]
fn long_time_limit_with_fast_provider_sends_only_turn_warning_once() {
    let d = tempfile::tempdir().unwrap();
    let p = Busy {
        requests: Mutex::new(Vec::new()),
        start: Instant::now(),
        step_at: None,
        end_at: None,
    };
    let r = Runner {
        provider: &p,
        shell: &FastShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 25,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(600)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut st = State::new("g", "true");
    let (outcome, _) = r.run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);

    let turn_warn = "You have 10 turns left of 25. Wrap up now: make sure your work is saved before you run out: committed, or written to the file the task asks for. Then stop calling tools.";
    let reqs = p.requests.lock().unwrap();
    assert_eq!(reqs.len(), 25);
    let first = reqs
        .iter()
        .position(|msgs| count_user_messages_with(msgs, turn_warn) > 0)
        .expect("the turn warning was sent");
    assert_eq!(first, 15, "the 16th call is the first with ten turns left");
    assert_eq!(
        count_user_messages_with(reqs.last().unwrap(), turn_warn),
        1,
        "the turn warning must appear only once in history"
    );
    for msgs in reqs.iter() {
        assert_eq!(
            count_user_messages_with(msgs, "minutes left of"),
            0,
            "the time warning must never appear"
        );
    }
}
