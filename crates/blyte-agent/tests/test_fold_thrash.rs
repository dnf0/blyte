use std::collections::VecDeque;
use std::sync::Mutex;

use blyte_agent::context::Folded;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
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

const THRASH_NOTE: &str = "[blyte] Your recent tool results are being dropped from your context because each turn reads more than fits (about 2000 tokens stay verbatim). Read narrower ranges (at most 200 lines), one or two files per turn, and act on what you have read before reading more.";

fn write_big_file(dir: &std::path::Path) {
    let line = "x".repeat(52);
    let content: String = (1..=220).map(|i| format!("{i:04} {line}\n")).collect();
    std::fs::write(dir.join("big.txt"), content).unwrap();
}

fn read_reply(start_line: usize, end_line: usize) -> Response {
    Response {
        text: "reading".into(),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: json!({
                "path": "big.txt",
                "start_line": start_line,
                "end_line": end_line,
            }),
            signature: None,
        }],
        finish_reason: Some("STOP".into()),
        ..Default::default()
    }
}

#[test]
fn big_reads_every_turn_warn_on_third_turn_and_stop_after_fifth_thrash() {
    let d = tempfile::tempdir().unwrap();
    write_big_file(d.path());

    let replies = (1..=10).map(|i| read_reply(i, i + 199)).collect();
    let provider = ScriptedProvider::new(replies);
    let context = Folded {
        budget_tokens: 2_000,
    };
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &context,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("goal", "");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(report.fold_thrash);
    assert_eq!(report.big_read_turns, 5);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|m| matches!(m, Message::User(s) if s == THRASH_NOTE)),
        "thrash note must not appear in 2nd turn's prompt"
    );
    assert!(
        requests[2]
            .messages
            .iter()
            .any(|m| matches!(m, Message::User(s) if s == THRASH_NOTE)),
        "expected 3rd turn's prompt to contain thrash note, got {:?}",
        requests[2].messages
    );
}

#[test]
fn small_reads_every_turn_produce_no_note_and_no_thrash() {
    let d = tempfile::tempdir().unwrap();
    write_big_file(d.path());

    let mut replies: Vec<Response> = (1..=4).map(|i| read_reply(i * 5, i * 5 + 2)).collect();
    replies.push(Response {
        text: "done".into(),
        finish_reason: Some("STOP".into()),
        ..Default::default()
    });
    let provider = ScriptedProvider::new(replies);
    let context = Folded {
        budget_tokens: 2_000,
    };
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &context,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("goal", "");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert!(!report.fold_thrash);
    assert_eq!(report.big_read_turns, 0);

    let requests = provider.requests.lock().unwrap();
    for (idx, req) in requests.iter().enumerate() {
        assert!(
            !req.messages
                .iter()
                .any(|m| matches!(m, Message::User(s) if s.starts_with("[blyte]"))),
            "unexpected harness note on turn {}: {:?}",
            idx + 1,
            req.messages
        );
    }
}

#[test]
fn one_big_read_then_small_ones_warns_once_and_does_not_stop() {
    let d = tempfile::tempdir().unwrap();
    write_big_file(d.path());

    let replies = vec![
        read_reply(1, 200),
        read_reply(201, 203),
        read_reply(204, 206),
        read_reply(207, 209),
        Response {
            text: "done".into(),
            finish_reason: Some("STOP".into()),
            ..Default::default()
        },
    ];
    let provider = ScriptedProvider::new(replies);
    let context = Folded {
        budget_tokens: 2_000,
    };
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &context,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let mut state = State::new("goal", "");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert!(!report.fold_thrash);
    assert_eq!(report.big_read_turns, 1);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    let vol_notes_in_turn_2 = requests[1]
        .messages
        .iter()
        .filter(|m| {
            matches!(
                m,
                Message::User(s)
                    if s.starts_with("[blyte] Your last turn's tool output was about ")
                        && s.ends_with(" tokens, more than half of what stays in context (2000). Read narrower ranges so it is not dropped before you use it.")
            )
        })
        .count();
    assert_eq!(
        vol_notes_in_turn_2, 1,
        "expected read-volume note once in turn 2: {:?}",
        requests[1].messages
    );
    for req in requests.iter() {
        assert!(
            !req.messages
                .iter()
                .any(|m| matches!(m, Message::User(s) if s == THRASH_NOTE)),
            "thrash note must not appear: {:?}",
            req.messages
        );
    }
}
