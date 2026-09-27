//! Parallel dispatches share the machine: gates take a machine-wide slot, so three runs do not
//! all start a full build and test suite at once (2026-09-27).

use std::time::{Duration, Instant};

use blyte_agent::gate_slots::{acquire, slots};

const POLL: Duration = Duration::from_millis(20);

#[test]
fn one_slot_makes_the_second_gate_wait_for_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let (first, waited) = acquire(1, dir.path(), POLL).unwrap();
    assert!(waited < Duration::from_millis(100));
    let d = dir.path().to_path_buf();
    let second = std::thread::spawn(move || {
        let start = Instant::now();
        let (_g, waited) = acquire(1, &d, POLL).unwrap();
        (start.elapsed(), waited)
    });
    std::thread::sleep(Duration::from_millis(400));
    drop(first);
    let (elapsed, waited) = second.join().unwrap();
    assert!(elapsed >= Duration::from_millis(350), "{elapsed:?}");
    assert!(waited >= Duration::from_millis(300), "{waited:?}");
}

#[test]
fn two_slots_let_two_gates_run_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (a, _) = acquire(2, dir.path(), POLL).unwrap();
    let (b, waited) = acquire(2, dir.path(), POLL).unwrap();
    assert!(waited < Duration::from_millis(100), "{waited:?}");
    assert_ne!(a.index(), b.index());
}

#[test]
fn a_released_slot_is_reused() {
    let dir = tempfile::tempdir().unwrap();
    let (a, _) = acquire(1, dir.path(), POLL).unwrap();
    drop(a);
    let (_b, waited) = acquire(1, dir.path(), POLL).unwrap();
    assert!(waited < Duration::from_millis(100));
}

// The only test in this binary that reads or sets BLYTE_GATE_SLOTS.
#[test]
fn the_slot_count_defaults_to_at_least_one_and_can_be_set() {
    std::env::remove_var("BLYTE_GATE_SLOTS");
    assert!(slots() >= 1);
    std::env::set_var("BLYTE_GATE_SLOTS", "3");
    assert_eq!(slots(), 3);
    std::env::set_var("BLYTE_GATE_SLOTS", "0");
    assert!(slots() >= 1, "0 falls back to the default");
    std::env::remove_var("BLYTE_GATE_SLOTS");
}
