//! Waiting for a gate slot does not use up a run's time budget, and a machine has more than one
//! slot: runs queued 39 minutes behind one slot and hit their deadline (2026-10-01).

use blyte_agent::gate_slots;

#[test]
fn the_default_is_a_quarter_of_the_cores_and_at_least_two() {
    std::env::remove_var("BLYTE_GATE_SLOTS");
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    assert_eq!(gate_slots::slots(), (cores / 4).max(2));
    std::env::set_var("BLYTE_GATE_SLOTS", "1");
    assert_eq!(gate_slots::slots(), 1);
    std::env::remove_var("BLYTE_GATE_SLOTS");
}

use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper;

impl Provider for Stopper {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

#[test]
fn a_gate_waiting_for_a_slot_past_max_duration_still_runs_and_verifies() {
    std::thread::sleep(Duration::from_millis(100));

    let prev_in_gate = std::env::var_os(gate_slots::IN_GATE_ENV);
    std::env::remove_var(gate_slots::IN_GATE_ENV);
    let prev_slots = std::env::var_os("BLYTE_GATE_SLOTS");
    let prev_slot_dir = std::env::var_os("BLYTE_GATE_SLOT_DIR");

    let slot_dir = tempfile::tempdir().unwrap();
    std::env::set_var("BLYTE_GATE_SLOT_DIR", slot_dir.path());
    std::env::set_var("BLYTE_GATE_SLOTS", "1");
    gate_slots::enable();

    let (held_slot, _) =
        gate_slots::acquire(1, slot_dir.path(), Duration::from_millis(20)).unwrap();

    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name c && touch base && git add -A && git commit -qm base",
            d.path(),
            Duration::from_secs(30),
        )
        .unwrap();

    let p = Stopper;
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 3,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_millis(600)),
        },
        rollback: false,
        on_event: None,
    };
    let mut st = State::new("g", "true");

    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        drop(held_slot);
    });

    let start = Instant::now();
    let (outcome, report) = r.run(&mut st);
    releaser.join().unwrap();

    std::env::remove_var("BLYTE_GATE_SLOT_DIR");
    std::env::remove_var("BLYTE_GATE_SLOTS");
    if let Some(v) = prev_slots {
        std::env::set_var("BLYTE_GATE_SLOTS", v);
    }
    if let Some(v) = prev_slot_dir {
        std::env::set_var("BLYTE_GATE_SLOT_DIR", v);
    }
    if let Some(v) = prev_in_gate {
        std::env::set_var(gate_slots::IN_GATE_ENV, v);
    }

    assert_eq!(outcome, Outcome::Verified);
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "run waited for the slot: {:?}",
        start.elapsed()
    );
    assert!(
        !report.gate_wait_ms.is_empty() && report.gate_wait_ms[0] >= 300,
        "recorded gate wait ms: {:?}",
        report.gate_wait_ms
    );
}
