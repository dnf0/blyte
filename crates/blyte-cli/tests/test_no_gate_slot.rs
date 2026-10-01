//! A run with `--no-gate-slot` does not take a machine-wide gate slot.

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn no_gate_slot_flag_is_accepted_and_bypasses_slot_acquisition() {
    let d = tempfile::tempdir().unwrap();
    let slot_dir = tempfile::tempdir().unwrap();

    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .status()
        .unwrap()
        .success());

    // Acquire the only slot in a private slot directory.
    let (_held, _) =
        blyte_agent::gate_slots::acquire(1, slot_dir.path(), Duration::from_millis(20)).unwrap();

    // Running blyte with --no-gate-slot should not wait on the held slot.
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "--no-gate-slot",
            "--gate",
            "true",
            "--sandbox",
            "off",
            "--max-turns",
            "1",
            "say done",
        ])
        .current_dir(d.path())
        .env("BLYTE_GATE_SLOT_DIR", slot_dir.path())
        .env("BLYTE_GATE_SLOTS", "1")
        .env_remove(blyte_agent::gate_slots::IN_GATE_ENV)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();

    // The run fails fast due to missing API key, but it did NOT wait or fail on CLI parsing.
    assert!(start.elapsed() < Duration::from_secs(5));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("unexpected argument '--no-gate-slot'"));
}
