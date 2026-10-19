//! A gate of several lines is several steps: every step runs and every failure is reported
//! together. With `fmt && clippy && test`, a shadow run found the clippy failure only after
//! fixing the fmt one -- a whole extra gate round of 7 turns.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

/// Always stops (no tool calls), so every turn triggers the gate. Records each request.
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

/// Runs with the given gate and at most two gate attempts; returns the outcome, gate attempts,
/// the system prompt and the text of the last request (which carries the gate feedback).
fn run(d: &tempfile::TempDir, gate: &str) -> (Outcome, u32, String, String) {
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
            max_turns: 6,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut st = State::new("g", gate);
    let (outcome, rep) = r.run(&mut st);
    let seen = p.seen.lock().unwrap();
    let sys = seen[0].system.clone();
    let last = format!("{:?}", seen.last().unwrap().messages);
    (outcome, rep.gate_attempts, sys, last)
}

#[test]
fn every_failing_step_is_reported_in_one_message() {
    let d = repo();
    let (outcome, _, _, last) = run(
        &d,
        "echo fmt-is-wrong && exit 1\ntrue\necho clippy-is-wrong && exit 2",
    );
    assert_eq!(outcome, Outcome::GateFailed);
    assert!(last.contains("2 of 3 steps failed"), "{last:.600}");
    assert!(last.contains("fmt-is-wrong"), "{last:.600}");
    assert!(last.contains("clippy-is-wrong"), "{last:.600}");
}

#[test]
fn a_step_runs_even_after_an_earlier_one_fails() {
    let d = repo();
    run(&d, "false\ntouch later-step-ran");
    assert!(d.path().join("later-step-ran").exists());
}

#[test]
fn a_gate_passes_only_when_every_step_passes() {
    let d = repo();
    let (outcome, attempts, _, _) = run(&d, "true\ntrue\n");
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(attempts, 1);
    let (outcome, _, _, _) = run(&d, "true\nfalse");
    assert_eq!(outcome, Outcome::GateFailed);
}

#[test]
fn a_single_line_gate_is_reported_as_before() {
    let d = repo();
    let (_, _, _, last) = run(&d, "echo plain && exit 3");
    assert!(last.contains("exit 3"), "{last:.600}");
    assert!(last.contains("plain"));
    assert!(!last.contains("steps failed"));
}

#[test]
fn the_system_prompt_shows_the_steps_as_one_command() {
    let d = repo();
    let (_, _, sys, _) = run(&d, "cargo fmt --check\ncargo test");
    assert!(
        sys.contains("`cargo fmt --check && cargo test`"),
        "{sys:.800}"
    );
}
