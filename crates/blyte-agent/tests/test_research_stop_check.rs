//! Research runs quote-check `report.md` even when they stop without passing the gate.

use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct ScriptedProvider {
    turn: Mutex<usize>,
    report_content: Option<String>,
}

impl Provider for ScriptedProvider {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        let turn = *t;

        if turn == 1 {
            if let Some(ref content) = self.report_content {
                return Ok(Response {
                    tool_calls: vec![ToolCall {
                        id: format!("c{turn}"),
                        name: "write".into(),
                        arguments: json!({"path": "report.md", "content": content}),
                        signature: None,
                    }],
                    ..Default::default()
                });
            }
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
fn research_stop_check_flags_mismatched_quote_on_budget_exhausted() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider {
        turn: Mutex::new(0),
        report_content: Some(
            "Entry point, src.py:1-2:\n```python\ndef main():\n    validate_all()\n```\n".into(),
        ),
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
    };

    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::BudgetExhausted), "{outcome:?}");
    let problems = report
        .quote_problems
        .expect("quote check should have run on stop");
    assert!(!problems.is_empty(), "expected quote problems, got empty");
    assert!(
        problems[0].contains("src.py:1-2"),
        "unexpected problem: {problems:?}"
    );
}

#[test]
fn research_stop_check_passes_matching_quote_on_budget_exhausted() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider {
        turn: Mutex::new(0),
        report_content: Some(
            "Entry point, src.py:1-2:\n```python\ndef main():\n    run()\n```\n".into(),
        ),
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
    };

    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::BudgetExhausted), "{outcome:?}");
    assert_eq!(report.quote_problems, Some(vec![]));
}

#[test]
fn non_research_run_out_of_turns_has_no_quote_check() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider {
        turn: Mutex::new(0),
        report_content: Some(
            "Entry point, src.py:1-2:\n```python\ndef main():\n    validate_all()\n```\n".into(),
        ),
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
        research: false,
    };

    let mut st = State::new("task", "true");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::BudgetExhausted), "{outcome:?}");
    assert_eq!(report.quote_problems, None);
}

#[test]
fn research_run_without_report_has_no_quote_check() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider {
        turn: Mutex::new(0),
        report_content: None,
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
    };

    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::BudgetExhausted), "{outcome:?}");
    assert_eq!(report.quote_problems, None);
}

#[test]
fn verified_research_run_records_empty_quote_problems() {
    struct Finisher(Mutex<usize>);
    impl Provider for Finisher {
        fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
            let mut t = self.0.lock().unwrap();
            *t += 1;
            if *t == 1 {
                Ok(Response {
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "write".into(),
                        arguments: json!({
                            "path": "report.md",
                            "content": "Entry point, src.py:1-2:\n```python\ndef main():\n    run()\n```\n"
                        }),
                        signature: None,
                    }],
                    ..Default::default()
                })
            } else {
                Ok(Response {
                    text: "done".into(),
                    ..Default::default()
                })
            }
        }
        fn model_id(&self) -> String {
            "scripted".into()
        }
    }

    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = Finisher(Mutex::new(0));
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
    };

    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = runner.run(&mut st);
    assert!(matches!(outcome, Outcome::Verified), "{outcome:?}");
    assert_eq!(report.quote_problems, Some(vec![]));
}
