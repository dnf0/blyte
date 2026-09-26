//! The help text and crate description describe blyte as it is, not the removed state engine.

use std::process::Command;

#[test]
fn help_describes_the_harness() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.contains("A lean coding-agent harness with a sealed verification gate"),
        "{help}"
    );
    assert!(!help.contains("deterministic state"), "{help}");
}

#[test]
fn the_crate_description_no_longer_mentions_a_state_engine() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains(
        "description = \"The blyte coding-agent harness: headless runs with a sealed verification gate, and a REPL.\""
    ));
    assert!(!manifest.contains("state engine"));
}
