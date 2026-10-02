//! Research runs periodically remind the model to keep report.md updated.

use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

const REMINDER: &str =
    "Update report.md now with what you have found so far, even if unfinished, then carry on.";

fn count_reminders(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| match m {
            Message::User(text) => text.contains(REMINDER),
            _ => false,
        })
        .count()
}

struct ScriptedAgent {
    turn: Mutex<usize>,
    seen: Mutex<Vec<Request>>,
    update_at_turn: Option<usize>,
}

impl Provider for ScriptedAgent {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        let turn = *t;

        if Some(turn) == self.update_at_turn {
            Ok(Response {
                tool_calls: vec![ToolCall {
                    id: format!("c{turn}"),
                    name: "write".into(),
                    arguments: json!({"path": "report.md", "content": "# Research Findings\n"}),
                    signature: None,
                }],
                ..Default::default()
            })
        } else {
            Ok(Response {
                tool_calls: vec![ToolCall {
                    id: format!("c{turn}"),
                    name: "bash".into(),
                    arguments: json!({"command": "true"}),
                    signature: None,
                }],
                ..Default::default()
            })
        }
    }

    fn model_id(&self) -> String {
        "scripted".into()
    }
}

#[test]
fn research_reminder_at_turn_30_and_not_again_at_60_if_report_changed() {
    let d = tempfile::tempdir().unwrap();

    let provider = ScriptedAgent {
        turn: Mutex::new(0),
        seen: Mutex::new(Vec::new()),
        update_at_turn: Some(40),
    };

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 65,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(30)),
        },
        rollback: false,
        on_event: None,
        research: true,
    };

    let mut st = State::new("research task", "true");
    let (_outcome, report) = runner.run(&mut st);

    let seen = provider.seen.lock().unwrap();
    assert!(seen.len() >= 60, "expected at least 60 turns");

    // Turns 1..29 must not have the reminder
    for (i, req) in seen.iter().enumerate().take(29) {
        assert_eq!(
            count_reminders(&req.messages),
            0,
            "turn {} should not have the reminder",
            i + 1
        );
    }

    // Turn 30 must have received the reminder
    assert_eq!(
        count_reminders(&seen[29].messages),
        1,
        "turn 30 should receive the reminder"
    );

    // At turn 40, report.md was updated by the write tool call.
    // At turn 60, the reminder must NOT have been added again.
    assert_eq!(
        count_reminders(&seen[59].messages),
        1,
        "turn 60 should not receive another reminder because report.md changed in between"
    );

    // The reminder does not count as a gate attempt
    assert_eq!(report.gate_attempts, 0);
}

#[test]
fn research_reminder_repeats_at_turn_60_if_report_never_changed() {
    let d = tempfile::tempdir().unwrap();

    let provider = ScriptedAgent {
        turn: Mutex::new(0),
        seen: Mutex::new(Vec::new()),
        update_at_turn: None,
    };

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 65,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(30)),
        },
        rollback: false,
        on_event: None,
        research: true,
    };

    let mut st = State::new("research task", "true");
    let (_outcome, report) = runner.run(&mut st);

    let seen = provider.seen.lock().unwrap();
    assert!(seen.len() >= 60, "expected at least 60 turns");

    // Turn 30 receives the first reminder
    assert_eq!(
        count_reminders(&seen[29].messages),
        1,
        "turn 30 should receive first reminder"
    );

    // Turn 60 receives the second reminder because report.md never changed
    assert_eq!(
        count_reminders(&seen[59].messages),
        2,
        "turn 60 should receive second reminder"
    );

    assert_eq!(report.gate_attempts, 0);
}
