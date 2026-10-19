//! Tool event lines show the command in full (up to 2000 characters) instead of cutting at 70.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Event, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct Scripted(Mutex<Vec<Response>>);

impl Provider for Scripted {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut s = self.0.lock().unwrap();
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

fn run_bash(cmd: &str) -> String {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted(Mutex::new(vec![Response {
        tool_calls: vec![ToolCall {
            id: "b1".into(),
            name: "bash".into(),
            arguments: json!({"command": cmd}),
            signature: None,
        }],
        ..Default::default()
    }]));
    let inputs: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let on_event = |e: Event<'_>| {
        if let Event::Tool { input, .. } = e {
            inputs.lock().unwrap().push(input);
        }
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 3,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: Some(&on_event),
        research: false,
        claim_check: false,
    };
    runner.run(&mut State::new("bash", "true"));
    inputs.into_inner().unwrap().remove(0)
}

#[test]
fn bash_call_150_char_single_line_equals_exactly() {
    let cmd = format!("echo '{}'", "a".repeat(143));
    assert_eq!(cmd.chars().count(), 150);
    let input = run_bash(&cmd);
    assert_eq!(input, cmd);
}

#[test]
fn bash_call_multiline_replaces_newlines() {
    let cmd = "echo one\necho two\n";
    let input = run_bash(cmd);
    assert_eq!(input, "echo one ↵ echo two");
}

#[test]
fn bash_call_2500_char_cut_at_2000_with_ellipsis() {
    let cmd = format!("echo '{}'", "a".repeat(2493));
    assert_eq!(cmd.chars().count(), 2500);
    let input = run_bash(&cmd);
    assert_eq!(input.chars().count(), 2001);
    assert!(input.ends_with('…'));
    let first_2000_input: String = input.chars().take(2000).collect();
    let first_2000_cmd: String = cmd.chars().take(2000).collect();
    assert_eq!(first_2000_input, first_2000_cmd);
}
