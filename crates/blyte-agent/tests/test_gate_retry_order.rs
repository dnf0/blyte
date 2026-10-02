//! A retry runs the previously failing gate steps first and stops there if they still fail:
//! re-running a passing full test suite while clippy still fails costs minutes and says nothing
//! new (2026-09-27).

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{retry_order, Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    seen: Mutex<Vec<Request>>,
}

impl Provider for Stopper {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name c && \
             touch base && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    d
}

fn run(d: &tempfile::TempDir, gate: &str, attempts: u32) -> (Outcome, String) {
    let p = Stopper {
        seen: Mutex::new(Vec::new()),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 8,
            max_gate_attempts: attempts,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
    };
    let mut st = State::new("g", gate);
    let (outcome, _) = r.run(&mut st);
    let last = format!("{:?}", p.seen.lock().unwrap().last().unwrap().messages);
    (outcome, last)
}

#[test]
fn retry_order_puts_the_failed_steps_first() {
    let steps = ["fmt", "clippy", "test"];
    assert_eq!(
        retry_order(&steps, &[]),
        (vec!["fmt", "clippy", "test"], vec![])
    );
    assert_eq!(
        retry_order(&steps, &["clippy".to_string()]),
        (vec!["clippy"], vec!["fmt", "test"])
    );
}

#[test]
fn a_step_that_passed_is_not_rerun_while_the_failing_one_still_fails() {
    let d = repo();
    // Step 1 always fails; step 2 passes and records every run.
    let (outcome, last) = run(&d, "echo still-broken && exit 1\necho ran >> slow.log", 3);
    assert_eq!(outcome, Outcome::GateFailed);
    let runs = std::fs::read_to_string(d.path().join("slow.log")).unwrap();
    assert_eq!(
        runs.lines().count(),
        1,
        "the passing step ran on the first attempt only"
    );
    assert!(last.contains("1 of 2 steps failed"), "{last:.600}");
    assert!(
        last.contains("Not re-run this attempt: 1 step(s) that passed last time"),
        "{last:.600}"
    );
}

#[test]
fn once_the_failing_step_passes_the_rest_runs_again() {
    let d = repo();
    // Step 1 fails on the first attempt only (it creates its own fix); step 2 records each run.
    let gate = "test -f fixed || (touch fixed && exit 1)\necho ran >> slow.log";
    let (outcome, _) = run(&d, gate, 3);
    assert_eq!(outcome, Outcome::Verified);
    let runs = std::fs::read_to_string(d.path().join("slow.log")).unwrap();
    assert_eq!(
        runs.lines().count(),
        2,
        "first attempt, then again once step 1 passed"
    );
}
