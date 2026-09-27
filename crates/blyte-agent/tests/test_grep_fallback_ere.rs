//! Without rg, the grep tool's fallback uses extended regex: basic `grep` read `|` and `( )`
//! literally, so `(AKIA|hf_)…` found nothing in a container that held the secrets (2026-09-27).

use std::time::Duration;

use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::tools::grep_command;

#[test]
fn the_fallback_asks_for_extended_regex() {
    let cmd = grep_command(false, "(a|b)+", ".", "");
    assert!(cmd.starts_with("grep -rnIE "), "{cmd}");
}

#[test]
fn alternation_and_repetition_match_through_the_fallback() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f.txt"), "nothing\ntoken hf_abc123 here\n").unwrap();
    let cmd = grep_command(false, "(AKIA|hf_)[a-z0-9]+", ".", "");
    let out = LocalShell
        .exec(&cmd, d.path(), Duration::from_secs(30))
        .unwrap();
    assert!(out.stdout.contains("hf_abc123"), "{}", out.merged());
}

#[test]
fn the_rg_command_is_unchanged() {
    assert!(grep_command(true, "x", ".", "").starts_with("rg --line-number"));
}
