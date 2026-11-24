//! In a research run, once the wrap-up warning has been sent, every tool except `write` and `edit`
//! is closed until `report.md` has changed.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct StubShell;

impl Shell for StubShell {
    fn exec(&self, command: &str, cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        let trimmed = command.trim();
        let exit_code = if trimmed == "true" {
            0
        } else if let Some(file) = trimmed.strip_prefix("touch ") {
            std::fs::write(cwd.join(file.trim()), "").map_err(|e| e.to_string())?;
            0
        } else {
            1
        };
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code,
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

const CLOSED_MSG: &str = "[blyte] almost out of budget: write report.md now (write or edit). Other tools are closed until report.md has changed.";

struct ScriptedAgent {
    turn: Mutex<usize>,
    seen: Mutex<Vec<Request>>,
    write_turns: Vec<usize>,
    fixed_content: Option<String>,
}

impl ScriptedAgent {
    fn new(write_turns: Vec<usize>, fixed_content: Option<&str>) -> Self {
        Self {
            turn: Mutex::new(0),
            seen: Mutex::new(Vec::new()),
            write_turns,
            fixed_content: fixed_content.map(str::to_owned),
        }
    }
}

impl Provider for ScriptedAgent {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        let turn = *t;

        if self.write_turns.contains(&turn) {
            let content = self
                .fixed_content
                .clone()
                .unwrap_or_else(|| format!("# Findings {turn}\n"));
            Ok(Response {
                tool_calls: vec![ToolCall {
                    id: format!("c{turn}"),
                    name: "write".into(),
                    arguments: json!({"path": "report.md", "content": content}),
                    signature: None,
                }],
                ..Default::default()
            })
        } else {
            Ok(Response {
                tool_calls: vec![ToolCall {
                    id: format!("c{turn}"),
                    name: "bash".into(),
                    arguments: json!({"command": format!("touch m{turn}")}),
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

fn run_with(provider: &ScriptedAgent, research: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let runner = Runner {
        provider,
        shell: &StubShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 25,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(300)),
        },
        rollback: false,
        on_event: None,
        research,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    let mut st = State::new("research task", "true");
    let _ = runner.run(&mut st);
    d
}

#[test]
fn tools_close_after_the_warning_until_the_report_changes() {
    let provider = ScriptedAgent::new(vec![], None);
    let d = run_with(&provider, true);

    assert!(d.path().join("m15").exists(), "m15 should exist");
    for n in 16..=25 {
        assert!(
            !d.path().join(format!("m{n}")).exists(),
            "m{n} should not exist"
        );
    }

    let seen = provider.seen.lock().unwrap();
    let has_closed_msg = seen.iter().skip(16).any(|req| {
        req.messages.iter().any(|m| match m {
            Message::ToolResult { content, .. } | Message::User(content) => {
                content.contains(CLOSED_MSG)
            }
            _ => false,
        })
    });
    assert!(
        has_closed_msg,
        "expected a request after turn 16 to contain the closed-tools message"
    );
}

#[test]
fn an_early_stub_does_not_open_the_tools() {
    let provider = ScriptedAgent::new(vec![5], None);
    let d = run_with(&provider, true);

    assert!(d.path().join("m15").exists(), "m15 should exist");
    for n in [16, 20, 25] {
        assert!(
            !d.path().join(format!("m{n}")).exists(),
            "m{n} should not exist"
        );
    }
}

#[test]
fn writing_the_report_opens_the_tools_again() {
    let provider = ScriptedAgent::new(vec![18], None);
    let d = run_with(&provider, true);

    assert!(!d.path().join("m16").exists(), "m16 should not exist");
    assert!(!d.path().join("m17").exists(), "m17 should not exist");
    assert!(d.path().join("m19").exists(), "m19 should exist");
    assert!(d.path().join("m25").exists(), "m25 should exist");
}

#[test]
fn rewriting_the_same_bytes_does_not_open_the_tools() {
    let provider = ScriptedAgent::new(vec![5, 18], Some("# Stub\n"));
    let d = run_with(&provider, true);

    assert!(!d.path().join("m19").exists(), "m19 should not exist");
}

#[test]
fn an_implementation_run_is_not_affected() {
    let provider = ScriptedAgent::new(vec![], None);
    let d = run_with(&provider, false);

    assert!(d.path().join("m20").exists(), "m20 should exist");
}
