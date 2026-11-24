//! `blyte review` shows why a run was blocked or failed its check.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap()
}

fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("missing {heading:?} in:\n{text}"));
    let after = &text[start + heading.len()..];
    let end = after.find("\n== ").unwrap_or(after.len());
    &after[..end]
}

#[test]
fn review_shows_why_a_run_was_blocked_at_setup() {
    let r = repo();
    std::fs::create_dir_all(r.path().join(".wt")).unwrap();
    std::fs::write(
        r.path().join(".wt/BLK1.json"),
        serde_json::json!({
            "outcome": "blocked",
            "error": "branch wip/BLK1 already exists"
        })
        .to_string(),
    )
    .unwrap();

    let out = blyte(r.path(), &["review", "BLK1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report = section(&stdout, "== report\n");
    assert!(report.contains("outcome: blocked"), "{report}");
    assert!(
        report.contains("error: branch wip/BLK1 already exists"),
        "{report}"
    );
}

#[test]
fn review_shows_an_error_the_final_message_does_not_contain() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/ERR1", ".wt/ERR1"],
    );
    std::fs::write(
        r.path().join(".wt/ERR1.report.json"),
        serde_json::json!({
            "outcome": "error",
            "error": "provider timed out after 120s",
            "final_message": "stopped before completing the edit"
        })
        .to_string(),
    )
    .unwrap();

    let out = blyte(r.path(), &["review", "ERR1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report = section(&stdout, "== report\n");
    let lines: Vec<&str> = report.lines().collect();
    let outcome_idx = lines
        .iter()
        .position(|l| *l == "outcome: error")
        .expect("outcome line");
    let error_idx = lines
        .iter()
        .position(|l| *l == "error: provider timed out after 120s")
        .expect("error line");
    assert!(error_idx > outcome_idx, "{report}");
}

#[test]
fn review_does_not_repeat_an_error_already_in_the_final_message() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/ERR2", ".wt/ERR2"],
    );
    std::fs::write(
        r.path().join(".wt/ERR2.report.json"),
        serde_json::json!({
            "outcome": "error",
            "error": "rate limit exceeded",
            "final_message": "run failed: rate limit exceeded on turn 4"
        })
        .to_string(),
    )
    .unwrap();

    let out = blyte(r.path(), &["review", "ERR2"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report = section(&stdout, "== report\n");
    assert!(!report.contains("error: rate limit exceeded"), "{report}");
    assert!(
        report.contains("final_message: run failed: rate limit exceeded on turn 4"),
        "{report}"
    );
}

#[test]
fn review_shows_the_last_check_failure() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/GF1", ".wt/GF1"],
    );
    std::fs::write(
        r.path().join(".wt/GF1.report.json"),
        serde_json::json!({
            "outcome": "gate_failed",
            "gate_failures": [
                "exit 1\nfirst attempt failure",
                "exit 1\nsecond attempt failure\nFAILED tests/x.py::test_y"
            ]
        })
        .to_string(),
    )
    .unwrap();

    let out = blyte(r.path(), &["review", "GF1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let failures = section(&stdout, "== check failures\n");
    assert!(
        failures.starts_with("(2 failed attempts; the last one)\n"),
        "{failures}"
    );
    assert!(failures.contains("second attempt failure"), "{failures}");
    assert!(failures.contains("FAILED tests/x.py::test_y"), "{failures}");
    assert!(!failures.contains("first attempt failure"), "{failures}");
}
