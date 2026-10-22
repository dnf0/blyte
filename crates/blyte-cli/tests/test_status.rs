//! `blyte status` shows what a running dispatch is doing: until a dispatch exited the
//! orchestrator could not tell progress from a loop (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::parse_log;

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success());
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(
        d.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
            "--no-gpg-sign",
        ],
    );
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn the_log_parser_reads_turns_tools_and_gates() {
    let log = "[Turn 1] \n\x1b[31m● bash cargo test -> exit 101\x1b[0m\n[Turn 2] \n● edit src/lib.rs -> edited src/lib.rs\n✗ Gate failed (attempt 1): boom\n[Turn 3] done\n✓ Gate passed (attempt 2)\n";
    let p = parse_log(log);
    assert_eq!(p.turn, 3);
    assert_eq!(
        p.last_tool.as_deref(),
        Some("edit src/lib.rs -> edited src/lib.rs")
    );
    assert_eq!(p.gate_attempts, 2);
    assert!(p.gate_passed);
    let empty = parse_log("");
    assert_eq!((empty.turn, empty.gate_attempts), (0, 0));
    assert!(empty.last_tool.is_none());
}

#[test]
fn status_of_a_finished_dispatch_reports_its_outcome() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "x").unwrap();
    blyte(
        r.path(),
        &[
            "dispatch",
            "T1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert!(r.path().join(".wt/T1.start.json").exists());
    let out = blyte(r.path(), &["status", "T1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["state"], "finished");
    assert_eq!(v["outcome"], "error"); // no API key in the test environment
    for k in [
        "elapsed_s",
        "turn",
        "last_tool",
        "gate_attempts",
        "gate_passed",
        "cost_usd",
        "final_message",
    ] {
        assert!(v.get(k).is_some(), "{k} missing");
    }
}

#[test]
fn an_unknown_id_is_an_error() {
    let r = repo();
    assert_eq!(blyte(r.path(), &["status", "nope"]).status.code(), Some(2));
}
