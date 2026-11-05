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
fn same_read_every_turn_warns_on_third_and_stops_after_31_turns() {
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
    assert_eq!(report.outcome_turns, 31);
    assert_eq!(report.peak_read_repeats, 30);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 31);
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

#[test]
fn turn_8_reminder_sent_when_report_missing_and_not_when_it_has_content() {
    let reminder =
        "Start report.md now with what you have found so far, even if unfinished, then carry on.";

    // 1. report.md is missing -> reminder is sent on turn 8.
    let d_missing = tempfile::tempdir().unwrap();
    std::fs::write(d_missing.path().join("a.txt"), "hello\n").unwrap();
    let mut replies_missing: Vec<Response> = (1..=7).map(|t| read_response(t, "a.txt")).collect();
    replies_missing.push(Response {
        text: "done".into(),
        finish_reason: Some("STOP".into()),
        ..Default::default()
    });
    let provider_missing = ScriptedProvider::new(replies_missing);
    let runner_missing = Runner {
        provider: &provider_missing,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d_missing.path().to_path_buf(),
        budget: Budget {
            max_turns: 20,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };
    let mut state = State::new("goal", "");
    let (outcome, _) = runner_missing.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);

    let reqs_missing = provider_missing.requests.lock().unwrap();
    assert_eq!(reqs_missing.len(), 8);
    for (idx, req) in reqs_missing[..7].iter().enumerate() {
        assert!(
            !req.messages
                .iter()
                .any(|m| matches!(m, Message::User(t) if t.contains(reminder))),
            "turn {} should not have turn-8 reminder",
            idx + 1
        );
    }
    assert!(
        reqs_missing[7]
            .messages
            .iter()
            .any(|m| matches!(m, Message::User(t) if t.contains(reminder))),
        "turn 8 should have the start-report reminder when report.md is missing"
    );

    // 2. report.md has content -> reminder is not sent on turn 8.
    let d_present = tempfile::tempdir().unwrap();
    std::fs::write(d_present.path().join("a.txt"), "hello\n").unwrap();
    std::fs::write(d_present.path().join("report.md"), "# Partial findings\n").unwrap();
    let mut replies_present: Vec<Response> = (1..=7).map(|t| read_response(t, "a.txt")).collect();
    replies_present.push(Response {
        text: "done".into(),
        finish_reason: Some("STOP".into()),
        ..Default::default()
    });
    let provider_present = ScriptedProvider::new(replies_present);
    let runner_present = Runner {
        provider: &provider_present,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d_present.path().to_path_buf(),
        budget: Budget {
            max_turns: 20,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };
    let mut state = State::new("goal", "");
    let (outcome, _) = runner_present.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);

    let reqs_present = provider_present.requests.lock().unwrap();
    assert_eq!(reqs_present.len(), 8);
    for (idx, req) in reqs_present.iter().enumerate() {
        assert!(
            !req.messages
                .iter()
                .any(|m| matches!(m, Message::User(t) if t.contains(reminder))),
            "turn {} should not have turn-8 reminder when report.md has content",
            idx + 1
        );
    }
}

#[test]
fn research_20_repeat_warning_added_once_and_third_use_mentions_report_md() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("target.txt"), "hello world\n").unwrap();

    let mut replies: Vec<Response> = (1..=25).map(|t| read_response(t, "target.txt")).collect();
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
            max_turns: 40,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };

    let mut state = State::new("goal", "");
    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    assert!(!report.read_loop);
    assert_eq!(report.peak_read_repeats, 24);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 26);
    let results: Vec<&str> = requests[25]
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 25);

    // Third use of the same call (turn 3, index 2) mentions report.md instead of "and make your change".
    assert!(
        results[2].contains("[blyte] You have made this exact call 3 times"),
        "expected 3rd tool result to contain repeat note, got: {}",
        results[2]
    );
    assert!(
        results[2].ends_with("write what you need into report.md."),
        "expected research 3rd-use warning to end with report.md instruction, got: {}",
        results[2]
    );
    assert!(!results[2].contains("and make your change"));

    // 20-repeat warning is added on the 21st read (index 20, when repeats == 20) and only once.
    let warn_indices: Vec<usize> = results
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            c.contains("[blyte] You have re-read 20 files or searches")
                .then_some(i)
        })
        .collect();
    assert_eq!(
        warn_indices,
        vec![20],
        "expected 20-repeat warning only at index 20"
    );
    assert!(results[20].contains("earlier results were dropped to save space"));
    assert!(results[20].contains("report.md"));
    assert!(results[20].contains("30"));
}

#[test]
fn research_at_30_repeats_closes_reading_allows_write_and_ends_normally() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("target.txt"), "hello world\n").unwrap();

    // Turns 1..=31 reach 30 repeats.
    let mut replies: Vec<Response> = (1..=31).map(|t| read_response(t, "target.txt")).collect();
    // Grace turn 1 (turn 32): model tries to read again -> gets "reading is closed" error.
    replies.push(read_response(32, "target.txt"));
    // Grace turn 2 (turn 33): model writes report.md.
    replies.push(Response {
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_33".into(),
            name: "write".into(),
            arguments: json!({
                "path": "report.md",
                "content": "# Findings\nhello world\n",
            }),
            signature: None,
        }],
        finish_reason: Some("STOP".into()),
        ..Default::default()
    });
    // Grace turn 3 (turn 34): model stops calling tools.
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
            max_turns: 60,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };

    let mut state = State::new("goal", "test -s report.md");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::Verified);
    assert!(report.read_loop);
    assert_eq!(report.outcome_turns, 34);
    assert_eq!(report.gate_attempts, 1);
    assert_eq!(report.quote_problems, Some(vec![]));

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 34);

    // After turn 31 (in request for turn 32), the [blyte] user message is present.
    assert!(
        requests[31].messages.iter().any(|m| matches!(
            m,
            Message::User(t)
                if t.contains("[blyte]")
                    && t.contains("report.md")
                    && t.contains("open questions")
        )),
        "expected [blyte] stop-reading user message after 30 repeats"
    );

    // After turn 32's read call (in request for turn 33), the tool result is an error saying reading is closed.
    let closed_result = requests[32]
        .messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::ToolResult {
                id,
                content,
                is_error,
                ..
            } if id == "call_32" => Some((content.as_str(), *is_error)),
            _ => None,
        })
        .expect("call_32 tool result");
    assert!(closed_result.1, "expected call_32 to be an error");
    assert!(
        closed_result.0.contains("reading is closed"),
        "expected 'reading is closed' in error result, got: {}",
        closed_result.0
    );
    assert!(
        closed_result.0.contains("report.md"),
        "expected 'report.md' in error result, got: {}",
        closed_result.0
    );
}

#[test]
fn research_at_30_repeats_exhausts_budget_after_3_grace_turns() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("target.txt"), "hello world\n").unwrap();

    // Turns 1..=31 reach 30 repeats.
    let mut replies: Vec<Response> = (1..=31).map(|t| read_response(t, "target.txt")).collect();
    // Grace turns 1..=3 (turns 32..=34): keep calling tools (including a write, which must not reset grace).
    for turn in 32..=50 {
        replies.push(Response {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: format!("call_{turn}"),
                name: "write".into(),
                arguments: json!({
                    "path": "report.md",
                    "content": format!("draft {turn}\n"),
                }),
                signature: None,
            }],
            finish_reason: Some("STOP".into()),
            ..Default::default()
        });
    }

    let provider = ScriptedProvider::new(replies);
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 60,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };

    let mut state = State::new("goal", "test -s report.md");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(report.read_loop);
    assert_eq!(report.outcome_turns, 34);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 34);
}
