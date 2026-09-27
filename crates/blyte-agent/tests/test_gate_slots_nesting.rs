//! Gate slots never make blyte wait for itself: a run holding the only slot ran `cargo test`,
//! whose runner tests waited for that slot until the gate's 30-minute deadline (2026-09-27).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::gate_slots::{acquire, enabled, nested, IN_GATE_ENV};
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    seen: Mutex<u32>,
}

impl Provider for Stopper {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        *self.seen.lock().unwrap() += 1;
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

// One test: it sets process environment variables.
#[test]
fn the_library_takes_no_slot_and_gate_commands_are_marked() {
    assert!(
        !enabled(),
        "slots are off unless the blyte command turns them on"
    );

    // Hold the only slot of a private slot directory, as a running blyte gate would.
    let slot_dir = tempfile::tempdir().unwrap();
    std::env::set_var("BLYTE_GATE_SLOT_DIR", slot_dir.path());
    std::env::set_var("BLYTE_GATE_SLOTS", "1");
    let (_held, _) = acquire(1, slot_dir.path(), Duration::from_millis(20)).unwrap();

    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name c && touch base && git add -A && git commit -qm base",
            d.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    let p = Stopper {
        seen: Mutex::new(0),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 3,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    // The gate records whether it saw the marker.
    let mut st = State::new(
        "g",
        format!("test -n \"${IN_GATE_ENV}\" && touch saw-marker"),
    );
    let start = Instant::now();
    let (outcome, report) = r.run(&mut st);
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "the run must not wait for the held slot"
    );
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_wait_ms, vec![0]);
    assert!(
        d.path().join("saw-marker").exists(),
        "gate commands see {IN_GATE_ENV}"
    );

    // Commands outside the gate do not see it.
    let out = LocalShell
        .exec(
            &format!("echo \"[${IN_GATE_ENV}]\""),
            d.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(out.stdout.trim(), "[]");

    assert!(!nested());
    std::env::set_var(IN_GATE_ENV, "1");
    assert!(nested());
    std::env::remove_var(IN_GATE_ENV);
    std::env::remove_var("BLYTE_GATE_SLOT_DIR");
    std::env::remove_var("BLYTE_GATE_SLOTS");
}
