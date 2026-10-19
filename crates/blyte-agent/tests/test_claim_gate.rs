//! Research runs check `report.md`'s claims against their cited lines at the gate, with one fix round.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

struct ScriptedProvider {
    replies: Mutex<VecDeque<Response>>,
    requests: Mutex<Vec<Request>>,
}

impl ScriptedProvider {
    fn new(replies: impl IntoIterator<Item = Response>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
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

fn write_report(id: &str, content: &str) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: "write".into(),
            arguments: json!({"path": "report.md", "content": content}),
            signature: None,
        }],
        ..Default::default()
    }
}

fn finish() -> Response {
    Response {
        text: "done".into(),
        ..Default::default()
    }
}

fn checker_reply(verdict: &str, reason: &str) -> Response {
    Response {
        text: json!([{"id": 1, "verdict": verdict, "reason": reason}]).to_string(),
        usage: Usage {
            input: 10,
            cached: 2,
            output: 5,
            thinking: 1,
        },
        ..Default::default()
    }
}

fn runner<'a>(provider: &'a dyn Provider, root: &Path, claim_check: bool) -> Runner<'a> {
    Runner {
        provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 5,
            max_duration: Some(Duration::from_secs(30)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check,
    }
}

#[test]
fn contradicted_then_fixed_and_supported_verifies_with_no_flags() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider::new([
        // Worker turn 1: writes report with a contradicted claim
        write_report("c1", "In src.py:1-2, main calls validate_all.\n"),
        // Worker turn 2: finishes -> gate 1 runs
        finish(),
        // Checker at gate 1: flags claim 1 as contradicted
        checker_reply("contradicted", "line 2 calls run(), not validate_all()"),
        // Worker turn 3: rewrites report.md after seeing the gate failure
        write_report("c2", "In src.py:1-2, main calls run.\n"),
        // Worker turn 4: finishes -> gate 2 runs
        finish(),
        // Checker at gate 2: marks claim 1 as supported
        checker_reply("supported", ""),
    ]);

    let r = runner(&provider, d.path(), true);
    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = r.run(&mut st);
    assert!(matches!(outcome, Outcome::Verified), "{outcome:?}");
    assert_eq!(report.gate_attempts, 2);

    // The worker's turn 3 request (index 3: 0=turn1, 1=turn2, 2=checker1, 3=turn3) contains the flagged claim.
    let reqs = provider.requests.lock().unwrap();
    let turn3_user = reqs[3]
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::User(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        turn3_user.contains("report.md:1: In src.py:1-2, main calls validate_all."),
        "expected claim in turn 3 request, got: {turn3_user}"
    );
    assert!(
        turn3_user.contains("checker: line 2 calls run(), not validate_all()"),
        "expected checker reason in turn 3 request, got: {turn3_user}"
    );
    assert!(
        turn3_user.contains("These claims do not match the lines they cite."),
        "expected fix instruction in turn 3 request, got: {turn3_user}"
    );

    let cc = report.claim_check.expect("claim_check should be recorded");
    assert_eq!(cc.checked, 1);
    assert!(
        cc.contradicted.is_empty(),
        "expected no flags, got: {:?}",
        cc.contradicted
    );
    // Checker usage from both gate runs (2 * 10 = 20 input) added into report.usage
    assert_eq!(report.usage.input, 20);
}

#[test]
fn contradicted_twice_passes_second_gate_and_keeps_flag() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider::new([
        // Worker turn 1: writes report
        write_report("c1", "In src.py:1-2, main calls validate_all.\n"),
        // Worker turn 2: finishes -> gate 1 runs and fails
        finish(),
        // Checker at gate 1: contradicted
        checker_reply("contradicted", "line 2 calls run(), not validate_all()"),
        // Worker turn 3: finishes without fixing -> gate 2 runs and passes (one fix round only)
        finish(),
        // Checker at gate 2: still contradicted
        checker_reply("contradicted", "still calls run(), not validate_all()"),
    ]);

    let r = runner(&provider, d.path(), true);
    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = r.run(&mut st);
    assert!(matches!(outcome, Outcome::Verified), "{outcome:?}");
    assert_eq!(report.gate_attempts, 2);

    let cc = report.claim_check.expect("claim_check should be recorded");
    assert_eq!(cc.checked, 1);
    assert_eq!(cc.contradicted.len(), 1);
    assert_eq!(cc.contradicted[0].line, 1);
    assert_eq!(
        cc.contradicted[0].reason,
        "still calls run(), not validate_all()"
    );
}

#[test]
fn claim_check_false_makes_no_checker_call() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def main():\n    run()\n").unwrap();

    let provider = ScriptedProvider::new([
        write_report("c1", "In src.py:1-2, main calls validate_all.\n"),
        finish(),
    ]);

    let r = runner(&provider, d.path(), false);
    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = r.run(&mut st);
    assert!(matches!(outcome, Outcome::Verified), "{outcome:?}");
    assert!(report.claim_check.is_none());
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}
