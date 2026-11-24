//! The report keeps what each failed gate attempt said, and a multi-step summary names the failing
//! steps: a run's detour after "1 of 2 steps failed" could not be diagnosed from its log.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{
    append_sandbox_denial_hint, base_check_tail, cut_gate_output, run_gate_steps,
    sandbox_denial_hint, Budget, RunReport, Runner,
};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper(Mutex<u32>);

impl Provider for Stopper {
    fn complete(&self, _: &Request) -> Result<Response, ProviderError> {
        *self.0.lock().unwrap() += 1;
        Ok(Response::default())
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

fn run(gate: &str, attempts: u32) -> RunReport {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             touch x && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    let p = Stopper(Mutex::new(0));
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: attempts,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    r.run(&mut State::new("g", gate)).1
}

#[test]
fn each_failed_attempt_is_kept() {
    let rep = run("echo boom-marker && exit 1", 2);
    assert_eq!(rep.gate_failures.len(), 2);
    assert!(
        rep.gate_failures[0].contains("boom-marker"),
        "{:?}",
        rep.gate_failures
    );
}

#[test]
fn the_multi_step_summary_names_the_failing_steps() {
    let rep = run("true\necho boom-marker && exit 1", 1);
    let first = rep.gate_failures[0].lines().next().unwrap();
    assert_eq!(first, "1 of 2 steps failed: echo boom-marker && exit 1");
}

#[test]
fn long_output_keeps_the_head_and_the_tail() {
    // The failure is usually reported last; the first line names the failing steps.
    let rep = run(
        "i=1; while [ $i -le 300 ]; do echo \"line $i xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; i=$((i + 1)); done; echo final-failure-marker; exit 1",
        1,
    );
    let g = &rep.gate_failures[0];
    assert!(g.chars().count() <= 4_100, "{}", g.chars().count());
    assert!(g.contains("line 1 "), "head kept");
    assert!(g.contains("final-failure-marker"), "tail kept");
    assert!(g.contains("characters cut] …"), "the cut is marked");
    assert!(!g.contains("line 150 "), "the middle is dropped");
}

#[test]
fn multi_byte_output_is_cut_on_character_boundaries() {
    let rep = run(
        "i=1; while [ $i -le 300 ]; do echo 'é€é€é€é€é€é€é€é€é€é€'; i=$((i + 1)); done; exit 1",
        1,
    );
    let g = &rep.gate_failures[0];
    assert!(g.chars().count() <= 4_100);
    assert!(g.contains("characters cut] …"));
}

#[test]
fn a_passing_gate_adds_nothing() {
    assert!(run("true", 1).gate_failures.is_empty());
}

#[test]
fn each_gate_attempt_records_its_turn() {
    let rep = run("echo boom-marker && exit 1", 2);
    assert_eq!(rep.gate_turns.len(), rep.gate_attempts as usize);
    assert!(
        rep.gate_turns.windows(2).all(|w| w[0] < w[1]),
        "{:?}",
        rep.gate_turns
    );
    assert!(*rep.gate_turns.last().unwrap() <= rep.outcome_turns);
}

#[test]
fn the_hint_is_added_once_to_a_failed_check_that_was_denied() {
    let d = tempfile::tempdir().unwrap();
    let mut deadline = None;
    let res = run_gate_steps(
        &LocalShell,
        "echo 'cat: /x/.env: Operation not permitted'; exit 1",
        d.path(),
        &[],
        &mut deadline,
        |_| {},
        &std::time::Instant::now,
    );
    assert!(!res.passed);
    let hint = sandbox_denial_hint("OPERATION NOT PERMITTED").unwrap();
    assert_eq!(res.output.lines().last(), Some(hint));
    assert_eq!(
        res.failures.last().unwrap().output.lines().last(),
        Some(hint)
    );
    assert_eq!(res.output.matches(hint).count(), 1);

    let mut again = res.output.clone();
    append_sandbox_denial_hint(&mut again);
    assert_eq!(again, res.output);
    assert_eq!(again.matches(hint).count(), 1);
}

#[test]
fn a_passing_check_gets_no_hint() {
    let d = tempfile::tempdir().unwrap();
    let mut deadline = None;
    let res = run_gate_steps(
        &LocalShell,
        "echo 'cat: /x/.env: Operation not permitted'; exit 0",
        d.path(),
        &[],
        &mut deadline,
        |_| {},
        &std::time::Instant::now,
    );
    assert!(res.passed);
    let hint = sandbox_denial_hint("operation not permitted").unwrap();
    assert!(!res.output.contains(hint));
}

#[test]
fn a_failed_check_without_the_phrase_gets_no_hint() {
    let d = tempfile::tempdir().unwrap();
    let mut deadline = None;
    let res = run_gate_steps(
        &LocalShell,
        "echo 'assertion failed: 1 != 2'; exit 1",
        d.path(),
        &[],
        &mut deadline,
        |_| {},
        &std::time::Instant::now,
    );
    assert!(!res.passed);
    assert_eq!(sandbox_denial_hint(&res.output), None);
    let hint = sandbox_denial_hint("operation not permitted").unwrap();
    assert!(!res.output.contains(hint));
    assert!(!res.failures[0].output.contains(hint));
}

#[test]
fn the_hint_survives_the_stored_cut() {
    let mut raw = String::new();
    for i in 1..=5_000 {
        raw.push_str(&format!("line {i}: some check output payload\n"));
    }
    raw.push_str("cat: /x/.env: Operation not permitted");
    append_sandbox_denial_hint(&mut raw);
    let hint = sandbox_denial_hint(&raw).unwrap();
    let cut = cut_gate_output(&raw);
    assert_eq!(cut.lines().last(), Some(hint));
}

#[test]
fn the_hint_survives_the_base_check_tail() {
    let mut raw = String::new();
    for i in 1..=100 {
        raw.push_str(&format!("line {i}\n"));
    }
    raw.push_str("cat: /x/.env: Operation not permitted");
    append_sandbox_denial_hint(&mut raw);
    let hint = sandbox_denial_hint(&raw).unwrap();
    let tail = base_check_tail(&raw);
    assert_eq!(tail.lines().last(), Some(hint));
}
