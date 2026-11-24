//! Tool event lines show the command in full (up to 2000 characters) instead of cutting at 70.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Event, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct StubShell;

impl Shell for StubShell {
    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn exec_host(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.exec(command, cwd, timeout)
    }

    fn describe(&self) -> String {
        String::new()
    }

    fn available_binaries(&self, _cwd: &Path, _candidates: &[&'static str]) -> Vec<String> {
        Vec::new()
    }
}

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
        shell: &StubShell,
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
        base_check: blyte_agent::run::BaseCheck::None,
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

#[test]
fn blyte_tool_event_formats_command_and_background() {
    use blyte_agent::run::format_blyte_tool_input;

    assert_eq!(
        format_blyte_tool_input(&json!({"args": ["wait", "--any", "IS1"]})),
        Some("blyte wait --any IS1".to_string())
    );
    assert_eq!(
        format_blyte_tool_input(
            &json!({"args": ["dispatch", "T1", "task.md"], "background": true})
        ),
        Some("blyte dispatch T1 task.md (background)".to_string())
    );
    assert_eq!(
        format_blyte_tool_input(
            &json!({"args": ["dispatch", "T1", "task.md"], "background": false})
        ),
        Some("blyte dispatch T1 task.md".to_string())
    );
    assert_eq!(
        format_blyte_tool_input(&json!({"args": ["dispatch", "T1", "my task.md", ""]})),
        Some("blyte dispatch T1 \"my task.md\" \"\"".to_string())
    );

    let d = tempfile::tempdir().unwrap();
    let p = Scripted(Mutex::new(vec![Response {
        tool_calls: vec![
            ToolCall {
                id: "b1".into(),
                name: "blyte".into(),
                arguments: json!({"args": ["wait", "--any", "IS1"]}),
                signature: None,
            },
            ToolCall {
                id: "b2".into(),
                name: "blyte".into(),
                arguments: json!({"args": ["dispatch", "T1", "task.md"], "background": true}),
                signature: None,
            },
        ],
        ..Default::default()
    }]));
    let started: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let finished: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let on_event = |e: Event<'_>| match e {
        Event::ToolStart {
            name: "blyte",
            input,
        } => {
            started.lock().unwrap().push(input);
        }
        Event::Tool {
            name: "blyte",
            input,
            ..
        } => {
            finished.lock().unwrap().push(input);
        }
        _ => {}
    };
    let runner = Runner {
        provider: &p,
        shell: &StubShell,
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
        base_check: blyte_agent::run::BaseCheck::None,
    };
    runner.run(&mut State::new("blyte", "true"));
    let expected = vec![
        "blyte wait --any IS1".to_string(),
        "blyte dispatch T1 task.md (background)".to_string(),
    ];
    assert_eq!(started.into_inner().unwrap(), expected);
    assert_eq!(finished.into_inner().unwrap(), expected);
}
