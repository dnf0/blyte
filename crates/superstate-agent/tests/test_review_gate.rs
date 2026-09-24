//! The completeness review gate: after the verification command passes, an independent review of
//! the change against the goal may send the worker back, at most twice; any failure to review
//! accepts; only the final phase of a phased run is reviewed.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use superstate_agent::context::Conversational;
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Phase, Runner};
use superstate_agent::shell::{LocalShell, Shell};
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

struct Scripted {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Scripted {
    fn new(script: Vec<Response>) -> Self {
        Self {
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }
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

fn says(text: &str) -> Response {
    Response {
        text: text.into(),
        ..Default::default()
    }
}

fn write(path: &str) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: format!("w_{path}"),
            name: "write".into(),
            arguments: json!({"path": path, "content": "x\n"}),
            signature: None,
        }],
        ..Default::default()
    }
}

const BLOCKING: &str = r#"```json
{"summary": "incomplete", "findings": [{"severity": "High", "file": "src/lib.rs", "line_range": [1, 2],
 "problem": "separator suppression is not applied to the last row", "suggested_fix": "handle the final row"}],
 "has_blocking_issues": true}
```"#;
const CLEAN: &str = r#"{"summary": "complete", "findings": [], "has_blocking_issues": false}"#;
const LOW_ONLY: &str = r#"{"summary": "nits", "findings": [{"severity": "Low", "file": "a", "line_range": [1, 1],
 "problem": "naming", "suggested_fix": "rename"}], "has_blocking_issues": true}"#;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let sh = |c: &str| {
        let o = LocalShell
            .exec(c, d.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(o.exit_code, 0, "{c}: {}", o.merged());
    };
    sh("git init -q && git config user.email a@b && git config user.name c");
    std::fs::write(d.path().join("seed.txt"), "seed\n").unwrap();
    sh("git add -A && git commit --no-gpg-sign -qm base");
    d
}

fn runner<'a>(p: &'a Scripted, root: &Path) -> Runner<'a> {
    Runner {
        provider: p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 5,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    }
}

fn reviewed_state(verify: &str) -> State {
    let mut st = State::new("implement separator suppression for every row", verify);
    st.orchestrator.review = true;
    st
}

#[test]
fn a_blocking_review_sends_the_worker_back_then_a_clean_one_accepts() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("done"),
        says(BLOCKING),
        says("fixed the last row"),
        says(CLEAN),
    ]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (2, 1));

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 5);
    // Review requests carry no tools and the reviewer's own instructions.
    for i in [2, 4] {
        assert!(seen[i].tools.is_empty());
        assert!(seen[i]
            .system
            .contains("You review whether a code change fully accomplishes a task"));
        let Message::User(prompt) = &seen[i].messages[0] else {
            panic!("review prompt must be a user message");
        };
        assert!(prompt.contains("implement separator suppression for every row"));
        assert!(
            prompt.contains("a.txt"),
            "the diff must include the new file: {prompt}"
        );
    }
    // The worker's next turn sees the unmet requirement.
    let feedback = format!("{:?}", seen[3].messages);
    assert!(
        feedback.contains("found requirements it does not meet"),
        "{feedback}"
    );
    assert!(feedback.contains("separator suppression is not applied to the last row"));
}

#[test]
fn without_review_a_passing_gate_is_accepted_at_once() {
    let d = repo();
    let p = Scripted::new(vec![write("a.txt"), says("done")]);
    let mut st = State::new("anything", "true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.reviews, 0);
    assert_eq!(p.seen.lock().unwrap().len(), 2);
}

#[test]
fn an_unparseable_review_accepts() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("done"),
        says("I think it's fine"),
    ]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (1, 0));
}

#[test]
fn review_rounds_are_capped_then_the_exit_code_stands() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("done"),
        says(BLOCKING),
        says("done again"),
        says(BLOCKING),
        says("done a third time"),
    ]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (2, 2));
    assert_eq!(p.seen.lock().unwrap().len(), 6);
}

#[test]
fn an_empty_diff_is_not_reviewed() {
    let d = repo();
    let p = Scripted::new(vec![says("done")]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.reviews, 0);
    assert_eq!(p.seen.lock().unwrap().len(), 1);
}

#[test]
fn low_severity_findings_do_not_block() {
    let d = repo();
    let p = Scripted::new(vec![write("a.txt"), says("done"), says(LOW_ONLY)]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (1, 0));
}

#[test]
fn a_phased_run_reviews_only_its_final_phase() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("red done"),
        write("b.txt"),
        says("green done"),
        says(CLEAN),
    ]);
    let mut st = reviewed_state("");
    let phase = |id: &str, verify: &str| Phase {
        id: id.into(),
        instruction: format!("do {id}"),
        verify_cmd: verify.into(),
        scope_only: None,
        protect: None,
    };
    let (outcome, report, _) = runner(&p, d.path()).run_phases(
        &mut st,
        &[
            phase("red", "test -f a.txt"),
            phase("green", "test -f b.txt"),
        ],
    );
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.reviews, 1);
    assert!(st.orchestrator.review, "the caller's setting is restored");
}
