//! The report keeps what each failed gate attempt said, and a multi-step summary names the failing
//! steps: a run's detour after "1 of 2 steps failed" could not be diagnosed from its log.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, RunReport, Runner};
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
        "for i in $(seq 1 300); do echo \"line $i $(printf 'x%.0s' $(seq 1 60))\"; done; echo final-failure-marker; exit 1",
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
        "for i in $(seq 1 300); do echo 'é€é€é€é€é€é€é€é€é€é€'; done; exit 1",
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
