//! A research report's quote directives are expanded in place before the report is checked.

use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

/// Writes `report` on turn 1, then says it is done (`finish`) or keeps reading (`!finish`).
struct Scripted {
    turn: Mutex<usize>,
    report: String,
    finish: bool,
}

impl Provider for Scripted {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        let turn = *t;
        if turn == 1 {
            return Ok(Response {
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "write".into(),
                    arguments: json!({"path": "report.md", "content": self.report}),
                    signature: None,
                }],
                ..Default::default()
            });
        }
        if self.finish {
            return Ok(Response {
                text: "done".into(),
                ..Default::default()
            });
        }
        Ok(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{turn}"),
                name: "read".into(),
                arguments: json!({"path": "src.py"}),
                signature: None,
            }],
            ..Default::default()
        })
    }

    fn model_id(&self) -> String {
        "scripted".into()
    }
}

#[test]
fn directive_expands_on_disk_and_the_report_passes() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src.py");
    std::fs::write(&src, "def main():\n    run()\n").unwrap();
    let report = format!("Entry point:\n<<quote {}:1-2>>\n", src.display());

    let provider = Scripted {
        turn: Mutex::new(0),
        report,
        finish: true,
    };
    // The question names the file, as a real research question does, so its path counts as seen.
    let question = format!("research task about {}", src.display());
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(30)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };

    let mut st = State::new(&question, "test -s report.md");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::Verified), "{outcome:?}");
    assert_eq!(report.quote_problems, Some(vec![]));

    let on_disk = std::fs::read_to_string(d.path().join("report.md")).unwrap();
    assert!(on_disk.contains("def main():\n    run()\n"), "{on_disk}");
    assert!(!on_disk.contains("<<quote"), "{on_disk}");
}

#[test]
fn directive_for_missing_file_is_reported_on_stop() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();
    let missing = d.path().join("missing.py");
    let report = format!("Entry point:\n<<quote {}:1-2>>\n", missing.display());

    let provider = Scripted {
        turn: Mutex::new(0),
        report,
        finish: false,
    };
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 2,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(30)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: false,
    };

    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::BudgetExhausted), "{outcome:?}");
    let problems = report
        .quote_problems
        .expect("quote check should have run on stop");
    assert!(
        problems.iter().any(|p| p.contains("cannot read the file")),
        "unexpected problems: {problems:?}"
    );
    let on_disk = std::fs::read_to_string(d.path().join("report.md")).unwrap();
    assert!(on_disk.contains("<<quote"), "{on_disk}");
}
