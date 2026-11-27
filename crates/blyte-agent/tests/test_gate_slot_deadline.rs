//! Waiting for a machine-wide gate slot does not eat into the run's `--max-seconds` budget.

use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::gate_slots;
use blyte_agent::run::{Budget, Event, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, Usage};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn the_default_is_a_quarter_of_the_cores_and_at_least_two() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("BLYTE_GATE_SLOTS");
    std::env::remove_var("BLYTE_GATE_SLOTS");
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    assert_eq!(gate_slots::slots(), (cores / 4).max(2));
    std::env::set_var("BLYTE_GATE_SLOTS", "1");
    assert_eq!(gate_slots::slots(), 1);
    match prev {
        Some(v) => std::env::set_var("BLYTE_GATE_SLOTS", v),
        None => std::env::remove_var("BLYTE_GATE_SLOTS"),
    }
}

struct Stopper;

impl Provider for Stopper {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        Ok(Response {
            text: "done".into(),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: None,
        })
    }

    fn model_id(&self) -> String {
        "fake/stopper".into()
    }
}

struct FastShell;

impl Shell for FastShell {
    fn describe(&self) -> String {
        "fast shell".into()
    }

    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

struct GateClock {
    base: Instant,
    offset: Mutex<Duration>,
}

impl GateClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
        })
    }

    fn advance(&self, delta: Duration) {
        let mut g = self.offset.lock().unwrap();
        *g += delta;
    }

    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap()
    }
}

#[test]
fn a_gate_waiting_for_a_slot_past_max_duration_still_runs_and_verifies() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const BUDGET: Duration = Duration::from_millis(80);
    const HOLD: Duration = Duration::from_millis(400);

    let d = tempfile::tempdir().unwrap();
    let slot_dir = tempfile::tempdir().unwrap();

    let prev_slots = std::env::var_os("BLYTE_GATE_SLOTS");
    let prev_dir = std::env::var_os("BLYTE_GATE_SLOT_DIR");
    let prev_in_gate = std::env::var_os(gate_slots::IN_GATE_ENV);
    std::env::remove_var(gate_slots::IN_GATE_ENV);
    std::env::set_var("BLYTE_GATE_SLOTS", "1");
    std::env::set_var("BLYTE_GATE_SLOT_DIR", slot_dir.path());

    gate_slots::enable();

    let (held_slot, _) =
        gate_slots::acquire(1, slot_dir.path(), Duration::from_millis(20)).unwrap();

    let (tx, rx) = mpsc::channel::<()>();
    let releaser = std::thread::spawn(move || {
        if rx.recv().is_ok() {
            std::thread::sleep(HOLD);
        }
        drop(held_slot);
    });

    let clock = GateClock::new();
    let clock_for_event = Arc::clone(&clock);
    let tx = Mutex::new(Some(tx));
    let on_event = |e: Event<'_>| match e {
        Event::GateStart { .. } => {
            if let Some(sender) = tx.lock().unwrap().take() {
                let _ = sender.send(());
            }
        }
        Event::GateSlotWait { ms } => {
            clock_for_event.advance(Duration::from_millis(ms));
        }
        _ => {}
    };

    let p = Stopper;
    let r = Runner {
        provider: &p,
        shell: &FastShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: Some(BUDGET),
        },
        rollback: false,
        on_event: Some(&on_event),
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };

    let mut st = State::new("finish", "true");
    let start = Instant::now();
    let (outcome, report) = r.run_with_clock(&mut st, &|| clock.now());
    let _ = tx.lock().unwrap().take();
    releaser.join().unwrap();

    match prev_slots {
        Some(v) => std::env::set_var("BLYTE_GATE_SLOTS", v),
        None => std::env::remove_var("BLYTE_GATE_SLOTS"),
    }
    match prev_dir {
        Some(v) => std::env::set_var("BLYTE_GATE_SLOT_DIR", v),
        None => std::env::remove_var("BLYTE_GATE_SLOT_DIR"),
    }
    match prev_in_gate {
        Some(v) => std::env::set_var(gate_slots::IN_GATE_ENV, v),
        None => std::env::remove_var(gate_slots::IN_GATE_ENV),
    }

    assert_eq!(outcome, Outcome::Verified);
    assert!(
        start.elapsed() >= BUDGET,
        "run finished too quickly: {:?}",
        start.elapsed()
    );
    assert_eq!(report.gate_wait_ms.len(), 1);
    assert!(
        report.gate_wait_ms[0] >= BUDGET.as_millis() as u64,
        "gate_wait_ms was {:?}",
        report.gate_wait_ms
    );
}
