use std::sync::{Arc, Mutex};
use std::time::Duration;

use blyte_agent::context::Folded;
use blyte_agent::fs::LocalFs as RealFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell as RealShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

const NUDGE_PREFIX: &str =
    "[blyte] You have spent 20 turns without changing any file or running the gate.";
const NUDGE_MARKER: &str =
    "turns without changing any file or running the gate. Stop exploring and decide now: make the change the task asks for, or, if the task needs no code change or the problem cannot be reproduced, stop calling tools and explain why in your reply.";
const LAST_CALL_TEXT: &str = "[blyte] Still no change after the last reminder. Reply now with your conclusion and why, and do not call any tools: the run stops if this reply calls a tool without changing anything.";

struct AlwaysBash {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    sleep: Option<Duration>,
}

impl Provider for AlwaysBash {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        if let Some(d) = self.sleep {
            std::thread::sleep(d);
        }
        self.requests.lock().unwrap().push(req.messages.clone());
        Ok(Response {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
                signature: None,
            }],
            usage: Usage::default(),
            finish_reason: None,
        })
    }

    fn model_id(&self) -> String {
        "stub".into()
    }
}

fn count_user_messages_with(messages: &[Message], needle: &str) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, Message::User(t) if t.contains(needle)))
        .count()
}

#[test]
fn fix_run_without_progress_nudges_at_21_last_calls_at_31_and_stops_at_32() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = AlwaysBash {
        requests: requests.clone(),
        sleep: None,
    };
    let runner = Runner {
        provider: &provider,
        context: &Folded::default(),
        fs: &RealFs,
        shell: &RealShell,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 60,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("fix the bug", "true");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(report.stalled);

    let reqs = requests.lock().unwrap();
    assert_eq!(
        reqs.len(),
        31,
        "stop must happen at top of turn 32 before the model call"
    );

    let first_nudge = reqs
        .iter()
        .position(|msgs| count_user_messages_with(msgs, NUDGE_PREFIX) > 0)
        .expect("nudge must appear");
    assert_eq!(first_nudge, 20, "nudge must first appear on turn 21");
    assert_eq!(
        count_user_messages_with(&reqs[20], NUDGE_PREFIX),
        1,
        "nudge must appear exactly once in turn 21 request"
    );
    assert_eq!(
        count_user_messages_with(&reqs[20], NUDGE_MARKER),
        1,
        "full nudge wording must match"
    );

    let first_last_call = reqs
        .iter()
        .position(|msgs| count_user_messages_with(msgs, LAST_CALL_TEXT) > 0)
        .expect("last call must appear");
    assert_eq!(
        first_last_call, 30,
        "last call must first appear on turn 31"
    );
    assert_eq!(
        count_user_messages_with(&reqs[30], LAST_CALL_TEXT),
        1,
        "last call must appear exactly once in turn 31 request"
    );
    assert_eq!(
        count_user_messages_with(&reqs[30], NUDGE_PREFIX),
        1,
        "nudge must still appear only once on turn 31"
    );
}

struct StopOnLastCall {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Provider for StopOnLastCall {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.requests.lock().unwrap().push(req.messages.clone());
        if count_user_messages_with(&req.messages, LAST_CALL_TEXT) > 0 {
            return Ok(Response {
                text: "No code change is needed because the behaviour is already compliant.".into(),
                tool_calls: Vec::new(),
                usage: Usage::default(),
                finish_reason: None,
            });
        }
        Ok(Response {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
                signature: None,
            }],
            usage: Usage::default(),
            finish_reason: None,
        })
    }

    fn model_id(&self) -> String {
        "stub".into()
    }
}

#[test]
fn replying_without_tools_after_last_call_stops_normally_without_stalling() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = StopOnLastCall {
        requests: requests.clone(),
    };
    let runner = Runner {
        provider: &provider,
        context: &Folded::default(),
        fs: &RealFs,
        shell: &RealShell,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 60,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("fix the bug", "");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert!(!report.stalled);
    assert_eq!(
        report.final_message.as_deref(),
        Some("No code change is needed because the behaviour is already compliant.")
    );
}

struct WriteOnTurn3ThenBash {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Provider for WriteOnTurn3ThenBash {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let turn = {
            let mut reqs = self.requests.lock().unwrap();
            reqs.push(req.messages.clone());
            reqs.len()
        };
        let call = if turn == 3 {
            ToolCall {
                id: "c1".into(),
                name: "write".into(),
                arguments: json!({"path": "changed.txt", "content": "done\n"}),
                signature: None,
            }
        } else {
            ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
                signature: None,
            }
        };
        Ok(Response {
            text: String::new(),
            tool_calls: vec![call],
            usage: Usage::default(),
            finish_reason: None,
        })
    }

    fn model_id(&self) -> String {
        "stub".into()
    }
}

#[test]
fn successful_write_disables_no_progress_nudge_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = WriteOnTurn3ThenBash {
        requests: requests.clone(),
    };
    let runner = Runner {
        provider: &provider,
        context: &Folded::default(),
        fs: &RealFs,
        shell: &RealShell,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 60,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("fix the bug", "true");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(!report.stalled);
    assert_eq!(report.outcome_turns, 60);

    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 60);
    for msgs in reqs.iter() {
        assert_eq!(count_user_messages_with(msgs, NUDGE_MARKER), 0);
        assert_eq!(count_user_messages_with(msgs, LAST_CALL_TEXT), 0);
    }
}

#[test]
fn research_run_never_receives_no_progress_nudge_or_stops() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = AlwaysBash {
        requests: requests.clone(),
        sleep: None,
    };
    let runner = Runner {
        provider: &provider,
        context: &Folded::default(),
        fs: &RealFs,
        shell: &RealShell,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 40,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };

    let mut state = State::new("investigate the repo", "");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(!report.stalled);

    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 40);
    for msgs in reqs.iter() {
        assert_eq!(count_user_messages_with(msgs, NUDGE_MARKER), 0);
        assert_eq!(count_user_messages_with(msgs, LAST_CALL_TEXT), 0);
    }
}

#[test]
fn time_fraction_triggers_nudge_early_and_last_call_before_end() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = AlwaysBash {
        requests: requests.clone(),
        sleep: Some(Duration::from_millis(100)),
    };
    let runner = Runner {
        provider: &provider,
        context: &Folded::default(),
        fs: &RealFs,
        shell: &RealShell,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 200,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(3)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("fix the bug", "true");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(report.stalled);

    let reqs = requests.lock().unwrap();
    let first_nudge_turn = reqs
        .iter()
        .position(|msgs| count_user_messages_with(msgs, NUDGE_MARKER) > 0)
        .map(|idx| idx + 1)
        .expect("nudge must appear");
    assert!(
        (9..=20).contains(&first_nudge_turn),
        "expected time-triggered nudge on turn in 9..=20, got turn {first_nudge_turn}"
    );
    assert_eq!(
        count_user_messages_with(&reqs[first_nudge_turn - 1], NUDGE_MARKER),
        1,
        "nudge must appear exactly once"
    );

    let first_last_call_turn = reqs
        .iter()
        .position(|msgs| count_user_messages_with(msgs, LAST_CALL_TEXT) > 0)
        .map(|idx| idx + 1)
        .expect("last call must appear before the run ends");
    assert!(
        first_last_call_turn > first_nudge_turn,
        "last call (turn {first_last_call_turn}) must appear after nudge (turn {first_nudge_turn})"
    );
    assert_eq!(
        count_user_messages_with(reqs.last().unwrap(), NUDGE_MARKER),
        1,
        "nudge must still appear only once at the end of the run"
    );
    assert_eq!(
        count_user_messages_with(reqs.last().unwrap(), LAST_CALL_TEXT),
        1,
        "last call must appear exactly once"
    );
}
