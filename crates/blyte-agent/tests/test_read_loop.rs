use std::collections::VecDeque;
use std::sync::Mutex;

use blyte_agent::context::Conversational;
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

fn read_response(turn: usize, path: &str) -> Response {
    Response {
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: format!("call_{turn}"),
            name: "read".into(),
            arguments: json!({ "path": path }),
            signature: None,
        }],
        finish_reason: Some("STOP".into()),
        ..Default::default()
    }
}

#[test]
fn same_read_every_turn_warns_on_third_and_stops_after_61_turns() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("target.txt"), "hello world\n").unwrap();

    let replies = (1..=200).map(|t| read_response(t, "target.txt")).collect();
    let provider = ScriptedProvider::new(replies);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 200,
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
    assert!(report.read_loop);
    assert_eq!(report.outcome_turns, 61);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 61);
    let results_after_3: Vec<&str> = requests[3]
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results_after_3.len(), 3);
    assert!(!results_after_3[0].contains("[blyte] You have made this exact call"));
    assert!(!results_after_3[1].contains("[blyte] You have made this exact call"));
    assert!(
        results_after_3[2].contains("[blyte] You have made this exact call 3 times"),
        "expected 3rd tool result to contain repeat note, got: {}",
        results_after_3[2]
    );
}

#[test]
fn write_every_second_turn_never_warns_or_stops_early() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("target.txt"), "hello world\n").unwrap();

    let replies = (1..=10)
        .map(|turn| {
            if turn % 2 == 1 {
                read_response(turn, "target.txt")
            } else {
                Response {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: format!("call_{turn}"),
                        name: "write".into(),
                        arguments: json!({
                            "path": "out.txt",
                            "content": format!("turn {turn}\n"),
                        }),
                        signature: None,
                    }],
                    finish_reason: Some("STOP".into()),
                    ..Default::default()
                }
            }
        })
        .collect();
    let provider = ScriptedProvider::new(replies);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
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
    assert!(!report.read_loop);
    assert_eq!(report.outcome_turns, 10);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 10);
    for req in requests.iter() {
        for msg in &req.messages {
            if let Message::ToolResult { content, .. } = msg {
                assert!(
                    !content.contains("[blyte] You have made this exact call"),
                    "unexpected repeat note: {content}"
                );
            }
        }
    }
}

#[test]
fn two_different_reads_alternating_warn_from_each_ones_third_occurrence() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "file a\n").unwrap();
    std::fs::write(d.path().join("b.txt"), "file b\n").unwrap();

    let mut replies: Vec<Response> = (1..=6)
        .map(|turn| {
            let path = if turn % 2 == 1 { "a.txt" } else { "b.txt" };
            read_response(turn, path)
        })
        .collect();
    replies.push(Response {
        text: "done".into(),
        finish_reason: Some("STOP".into()),
        ..Default::default()
    });
    let provider = ScriptedProvider::new(replies);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
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
    assert!(!report.read_loop);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 7);
    let results: Vec<&str> = requests[6]
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 6);
    for (idx, content) in results[..4].iter().enumerate() {
        assert!(
            !content.contains("[blyte] You have made this exact call"),
            "call {} should not have a repeat note: {content}",
            idx + 1
        );
    }
    assert!(
        results[4].contains("[blyte] You have made this exact call 3 times"),
        "expected 5th call (3rd read of a.txt) to have repeat note, got: {}",
        results[4]
    );
    assert!(
        results[5].contains("[blyte] You have made this exact call 3 times"),
        "expected 6th call (3rd read of b.txt) to have repeat note, got: {}",
        results[5]
    );
}
