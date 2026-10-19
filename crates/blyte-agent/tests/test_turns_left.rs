//! The model is told once, ten turns before the end, to save its work: runs ran out of turns
//! without writing their report or committing (2026-10-01).

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

/// Asks for a tool call every turn and records, per call, whether the request carried the warning.
struct Busy {
    seen: Mutex<Vec<bool>>,
}

impl Provider for Busy {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let text = format!("{:?}", req.messages);
        let mut seen = self.seen.lock().unwrap();
        seen.push(text.contains("You have 10 turns left of 30"));
        Ok(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{}", seen.len()),
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

#[test]
fn the_warning_arrives_once_ten_turns_before_the_end() {
    let d = tempfile::tempdir().unwrap();
    let p = Busy {
        seen: Mutex::new(Vec::new()),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
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
    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 30);
    let first = seen.iter().position(|s| *s).expect("the warning was sent");
    assert_eq!(first, 20, "the 21st call is the first with ten turns left");
}
