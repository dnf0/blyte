use std::path::Path;
use std::process::Command;

use blyte_cli::queue_actions::{
    clear_hold, decide, ensure_checks, read_hold, set_hold, Action, Decision, Hold,
};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.email", "t@e.com"]);
    git(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("README.md"), "hello\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "init"]);
    std::fs::create_dir_all(p.join(".wt")).unwrap();
    dir
}

fn write_code_run(root: &Path, id: &str, outcome: &str) {
    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join(id);
    let summary = serde_json::json!({
        "id": id,
        "worktree": wt_path.display().to_string(),
        "branch": format!("wip/{id}"),
        "outcome": outcome,
        "exit": if outcome == "verified" { 0 } else { 1 },
        "error": if outcome == "verified" { serde_json::Value::Null } else { serde_json::json!("failed") },
        "commits": [{"sha": "abc1234", "subject": "work"}],
        "changed_files": ["src/lib.rs"],
        "final_message": "done",
        "turns": 3,
        "gate_attempts": 1,
        "gate_failures": [],
        "cost_usd": 0.05,
        "model": "claude-sonnet-4-5",
        "seconds": 12,
        "sandbox": "none",
        "log": wt_dir.join(format!("{id}.log")).display().to_string(),
        "report": wt_dir.join(format!("{id}.report.json")).display().to_string(),
        "gate_source": "config",
        "research_report": serde_json::Value::Null,
    });
    std::fs::write(
        wt_dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&summary).unwrap() + "\n",
    )
    .unwrap();
}

fn write_research_run(root: &Path, id: &str, outcome: &str) {
    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join(id);
    let report_md = wt_dir.join(format!("{id}.report.md"));
    std::fs::write(&report_md, "# Report\n\nFindings.\n").unwrap();
    let summary = serde_json::json!({
        "id": id,
        "worktree": wt_path.display().to_string(),
        "branch": format!("wip/{id}"),
        "outcome": outcome,
        "exit": if outcome == "verified" { 0 } else { 1 },
        "error": serde_json::Value::Null,
        "commits": [],
        "changed_files": [],
        "final_message": "done",
        "turns": 4,
        "gate_attempts": 1,
        "gate_failures": [],
        "cost_usd": 0.04,
        "model": "claude-sonnet-4-5",
        "seconds": 10,
        "sandbox": "none",
        "log": wt_dir.join(format!("{id}.log")).display().to_string(),
        "report": wt_dir.join(format!("{id}.report.json")).display().to_string(),
        "gate_source": "research",
        "research_report": report_md.display().to_string(),
    });
    std::fs::write(
        wt_dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&summary).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn accept_verified_code_run() {
    let repo = init_repo();
    write_code_run(repo.path(), "T1", "verified");

    let decision = decide(repo.path(), "T1", Action::Accept, Some("  ship it  "), 1700).unwrap();
    assert_eq!(
        decision,
        Decision {
            run: "T1".to_string(),
            action: Action::Accept,
            note: Some("ship it".to_string()),
            orchestrator_message: None,
        }
    );

    let verdict_path = repo.path().join(".wt/T1.verdict.json");
    let verdict: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(verdict["verdict"], "accepted");
    assert_eq!(verdict["notes"], "ship it");
    assert_eq!(verdict["ts"], 1700);
}

#[test]
fn accept_refuses_research_run() {
    let repo = init_repo();
    write_research_run(repo.path(), "R1", "verified");

    let err = decide(repo.path(), "R1", Action::Accept, None, 1700).unwrap_err();
    assert_eq!(err, "run R1 is a research run; mark it read");
    assert!(!repo.path().join(".wt/R1.verdict.json").exists());
}

#[test]
fn accept_refuses_unverified_run() {
    let repo = init_repo();
    write_code_run(repo.path(), "T2", "gate_failed");

    let err = decide(repo.path(), "T2", Action::Accept, None, 1700).unwrap_err();
    assert_eq!(
        err,
        "run T2 is gate_failed, not verified; it cannot be accepted"
    );
    assert!(!repo.path().join(".wt/T2.verdict.json").exists());
}

#[test]
fn mark_read_refuses_code_run() {
    let repo = init_repo();
    write_code_run(repo.path(), "T3", "verified");

    let err = decide(repo.path(), "T3", Action::MarkRead, None, 1700).unwrap_err();
    assert_eq!(err, "run T3 is not a research run");
    assert!(!repo.path().join(".wt/T3.verdict.json").exists());
}

#[test]
fn unknown_run_is_an_error() {
    let repo = init_repo();

    let err = decide(repo.path(), "NOPE", Action::Accept, None, 1700).unwrap_err();
    let expected = format!(
        "no finished run NOPE in {}",
        repo.path().join(".wt").display()
    );
    assert_eq!(err, expected);
}

#[test]
fn send_back_needs_a_note() {
    let repo = init_repo();
    write_code_run(repo.path(), "T4", "verified");

    let err_none = decide(repo.path(), "T4", Action::SendBack, None, 1700).unwrap_err();
    assert_eq!(err_none, "a note is required to send back run T4");

    let err_ws = decide(repo.path(), "T4", Action::SendBack, Some("  "), 1700).unwrap_err();
    assert_eq!(err_ws, "a note is required to send back run T4");
}

#[test]
fn send_back_records_no_verdict() {
    let repo = init_repo();
    write_code_run(repo.path(), "T5", "verified");

    let decision = decide(
        repo.path(),
        "T5",
        Action::SendBack,
        Some("  handle empty input.  "),
        1700,
    )
    .unwrap();
    assert_eq!(
        decision,
        Decision {
            run: "T5".to_string(),
            action: Action::SendBack,
            note: Some("handle empty input.".to_string()),
            orchestrator_message: Some(
                "Send back run T5: handle empty input. Draft a follow-up task for me to confirm."
                    .to_string()
            ),
        }
    );
    assert!(!repo.path().join(".wt/T5.verdict.json").exists());
}

#[test]
fn redo_records_redone() {
    let repo = init_repo();
    write_code_run(repo.path(), "T6", "gate_failed");

    let without_note = decide(repo.path(), "T6", Action::Redo, None, 1700).unwrap();
    assert_eq!(
        without_note,
        Decision {
            run: "T6".to_string(),
            action: Action::Redo,
            note: None,
            orchestrator_message: Some("Redo run T6.".to_string()),
        }
    );
    let v1: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.path().join(".wt/T6.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v1["verdict"], "redone");
    assert_eq!(v1["notes"], "");
    assert_eq!(v1["ts"], 1700);

    let with_note = decide(
        repo.path(),
        "T6",
        Action::Redo,
        Some("  wrong approach  "),
        1800,
    )
    .unwrap();
    assert_eq!(
        with_note,
        Decision {
            run: "T6".to_string(),
            action: Action::Redo,
            note: Some("wrong approach".to_string()),
            orchestrator_message: Some("Redo run T6. Reason: wrong approach".to_string()),
        }
    );
    let v2: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.path().join(".wt/T6.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v2["verdict"], "redone");
    assert_eq!(v2["notes"], "wrong approach");
    assert_eq!(v2["ts"], 1800);
}

#[test]
fn hold_round_trip() {
    let repo = init_repo();

    assert_eq!(read_hold(repo.path(), "H1").unwrap(), None);
    clear_hold(repo.path(), "H1").unwrap();

    set_hold(repo.path(), "H1", "  wait for API  ", 1900).unwrap();
    assert_eq!(
        read_hold(repo.path(), "H1").unwrap(),
        Some(Hold {
            note: "wait for API".to_string(),
            ts: 1900,
        })
    );

    clear_hold(repo.path(), "H1").unwrap();
    assert_eq!(read_hold(repo.path(), "H1").unwrap(), None);
}

#[test]
fn hold_needs_a_note() {
    let repo = init_repo();
    write_code_run(repo.path(), "H2", "verified");

    assert!(set_hold(repo.path(), "H2", "", 1900).is_err());
    assert!(set_hold(repo.path(), "H2", "   \t\n ", 1900).is_err());

    let err_none = decide(repo.path(), "H2", Action::Hold, None, 1900).unwrap_err();
    assert_eq!(err_none, "a note is required to hold run H2");

    let err_ws = decide(repo.path(), "H2", Action::Hold, Some("   "), 1900).unwrap_err();
    assert_eq!(err_ws, "a note is required to hold run H2");
}

#[test]
fn corrupt_hold_file_is_an_error() {
    let repo = init_repo();
    let hold_path = repo.path().join(".wt/H3.hold.json");
    std::fs::write(&hold_path, "{not json").unwrap();

    let err = read_hold(repo.path(), "H3").unwrap_err();
    assert!(
        err.contains(&hold_path.display().to_string()),
        "error should name the hold path: {err}"
    );
}

#[test]
fn decision_clears_hold() {
    let repo = init_repo();
    write_research_run(repo.path(), "R2", "verified");

    let held = decide(repo.path(), "R2", Action::Hold, Some("read later"), 2000).unwrap();
    assert_eq!(held.orchestrator_message, None);
    assert_eq!(held.note, Some("read later".to_string()));
    assert!(repo.path().join(".wt/R2.hold.json").exists());

    let read = decide(repo.path(), "R2", Action::MarkRead, None, 2100).unwrap();
    assert_eq!(read.orchestrator_message, None);
    assert!(!repo.path().join(".wt/R2.hold.json").exists());
    assert_eq!(read_hold(repo.path(), "R2").unwrap(), None);
}

#[test]
fn mark_read_records_accepted_with_note_read() {
    let repo = init_repo();
    write_research_run(repo.path(), "R3", "verified");

    let decision = decide(repo.path(), "R3", Action::MarkRead, None, 2200).unwrap();
    assert_eq!(
        decision,
        Decision {
            run: "R3".to_string(),
            action: Action::MarkRead,
            note: None,
            orchestrator_message: None,
        }
    );

    let verdict: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.path().join(".wt/R3.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(verdict["verdict"], "accepted");
    assert_eq!(verdict["notes"], "read");
    assert_eq!(verdict["ts"], 2200);
}

#[test]
fn ensure_checks_writes_once() {
    let repo = init_repo();
    let base_sha = git(repo.path(), &["rev-parse", "HEAD"]);
    let wt_path = repo.path().join(".wt/C1");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "wip/C1",
            wt_path.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(wt_path.join("work.txt"), "done\n").unwrap();
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-m", "feat: add work"]);

    std::fs::write(
        repo.path().join(".wt/C1.start.json"),
        serde_json::json!({ "id": "C1", "base_sha": base_sha }).to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.path().join(".wt/C1.report.json"),
        serde_json::json!({ "outcome": "verified", "turns": 2 }).to_string(),
    )
    .unwrap();
    write_code_run(repo.path(), "C1", "verified");

    let checks_path = repo.path().join(".wt/C1.checks.json");
    assert!(!checks_path.exists());

    assert!(ensure_checks(repo.path(), "C1").unwrap());
    assert!(checks_path.exists());

    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&checks_path).unwrap()).unwrap();
    assert!(parsed.get("ts").and_then(|v| v.as_u64()).is_some());
    assert!(parsed.get("flags").and_then(|v| v.as_array()).is_some());
    assert!(parsed.get("fallbacks").and_then(|v| v.as_array()).is_some());

    assert!(!ensure_checks(repo.path(), "C1").unwrap());
}

#[test]
fn ensure_checks_refuses_unfinished_run() {
    let repo = init_repo();
    std::fs::write(
        repo.path().join(".wt/U1.start.json"),
        serde_json::json!({ "id": "U1" }).to_string(),
    )
    .unwrap();

    let err = ensure_checks(repo.path(), "U1").unwrap_err();
    assert!(err.contains("U1"), "error should name the run: {err}");
    assert!(!repo.path().join(".wt/U1.checks.json").exists());
}
