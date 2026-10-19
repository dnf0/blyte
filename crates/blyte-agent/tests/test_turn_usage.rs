//! Usage is recorded per model call, with how the prompt changed since the previous call, so
//! fresh input can be split into new content and cache misses.

use std::sync::Mutex;

use blyte_agent::context::{ContextStrategy, Conversational, Folded};
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, RunReport, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

struct Scripted {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
        Ok(if s.is_empty() {
            Response::default()
        } else {
            s.remove(0)
        })
    }
    fn model_id(&self) -> String {
        "scripted".into()
    }
}

fn chars(m: &Message) -> usize {
    match m {
        Message::User(t) | Message::Assistant(t) => t.len(),
        Message::ToolCall { arguments, .. } => arguments.to_string().len(),
        Message::ToolResult { content, .. } => content.len(),
    }
}

fn usage(n: u64) -> Usage {
    Usage {
        input: 100 * n,
        cached: 10 * n,
        output: 5,
        thinking: 1,
    }
}

/// Three turns reading different 40-line files, then done.
fn run(context: &dyn ContextStrategy) -> (Outcome, RunReport, Vec<Request>) {
    let d = tempfile::tempdir().unwrap();
    let body = format!("{}\n", "x".repeat(50)).repeat(40);
    let mut script = Vec::new();
    for i in 0..3u64 {
        std::fs::write(d.path().join(format!("f{i}.txt")), &body).unwrap();
        script.push(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{i}"),
                name: "read".into(),
                arguments: json!({"path": format!("f{i}.txt")}),
                signature: None,
            }],
            usage: usage(i + 1),
            ..Default::default()
        });
    }
    script.push(Response {
        text: "done".into(),
        usage: usage(4),
        ..Default::default()
    });
    let p = Scripted {
        script: Mutex::new(script),
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context,
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
    let (outcome, report) = runner.run(&mut State::new("read the files", "true"));
    let seen = p.seen.lock().unwrap().clone();
    (outcome, report, seen)
}

#[test]
fn every_turn_records_its_usage_and_whether_the_prompt_only_grew() {
    let (outcome, report, seen) = run(&Conversational);
    assert_eq!(outcome, Outcome::Verified);
    let u = &report.turn_usage;
    assert_eq!(
        u.iter().map(|t| t.turn).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        (u[1].input, u[1].cached, u[1].output, u[1].thinking),
        (200, 20, 5, 1)
    );
    assert_eq!(report.usage.input, u.iter().map(|t| t.input).sum::<u64>());

    assert!(!u[0].appended, "the first call has nothing to extend");
    assert!(
        u[1..].iter().all(|t| t.appended),
        "conversational only appends"
    );
    for k in 1..4 {
        assert_eq!(u[k].prompt_messages, seen[k].messages.len());
        let added: usize = seen[k].messages[seen[k - 1].messages.len()..]
            .iter()
            .map(chars)
            .sum();
        assert_eq!(u[k].new_chars, added, "turn {}", k + 1);
    }
    let whole: usize = seen[0].messages.iter().map(chars).sum();
    assert_eq!(u[0].new_chars, whole);
}

#[test]
fn a_fold_is_recorded_as_a_rewritten_prompt() {
    let (_, report, _) = run(&Folded { budget_tokens: 300 });
    assert!(
        report.turn_usage.iter().skip(1).any(|t| !t.appended),
        "{:?}",
        report.turn_usage
    );
}
