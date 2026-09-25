//! `--tdd`: autonomous TDD as native-harness phases, and how the report records it.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use blyte_agent::run::{Outcome, PhaseResult};
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::{tdd_phases, TaskConfig};
use serde_json::json;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "fix it".into(),
        gate: None,
        context_strategy: "folded".into(),
        model: Some("gemini-3.8-flash-high".into()),
        container: None,
        max_turns: 1000,
        max_gates: 5,
        rollback: true,
        root: PathBuf::from("."),
        loop_breaker: true,
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        tdd: false,
        review: false,
        fold_budget: None,
    }
}

#[test]
fn tdd_phases_are_red_then_green_from_blyte_core() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pyproject.toml"), "").unwrap();
    let profile = blyte_core::tdd::detect_profile(d.path()).unwrap();
    let phases = tdd_phases(d.path()).unwrap();
    assert_eq!(phases.len(), 2);
    assert_eq!(phases[0].id, "tdd-reproduction-test");
    assert_eq!(phases[0].verify_cmd, profile.red_cmd);
    assert_eq!(
        phases[0].scope_only.as_deref(),
        Some(profile.reproduction_path)
    );
    assert_eq!(phases[0].protect, None);
    assert_eq!(phases[1].id, "fix-implementation");
    assert_eq!(phases[1].verify_cmd, profile.green_cmd);
    assert_eq!(phases[1].scope_only, None);
    assert_eq!(
        phases[1].protect.as_deref(),
        Some(profile.reproduction_path)
    );
}

#[test]
fn an_unrecognised_repository_is_an_error_never_ungated() {
    let d = tempfile::tempdir().unwrap();
    let err = tdd_phases(d.path()).unwrap_err();
    assert!(err.contains("autonomous TDD"), "{err}");
}

#[test]
fn gate_source_names_where_the_gate_came_from() {
    let none = RunRecord::from_error(&config(), "x", Duration::ZERO);
    assert_eq!((none.gate_source.as_str(), none.gated), ("none", false));

    let mut user = config();
    user.gate = Some("pytest".into());
    let user = RunRecord::from_error(&user, "x", Duration::ZERO);
    assert_eq!((user.gate_source.as_str(), user.gated), ("user", true));

    let mut tdd = config();
    tdd.tdd = true;
    let tdd = RunRecord::from_error(&tdd, "x", Duration::ZERO);
    assert_eq!((tdd.gate_source.as_str(), tdd.gated), ("tdd", true));
}

#[test]
fn phase_results_are_recorded_in_order() {
    let r = RunRecord::from_error(&config(), "x", Duration::ZERO).with_phases(&[
        PhaseResult {
            id: "tdd-reproduction-test".into(),
            outcome: Outcome::Verified,
            turns: 3,
            gate_attempts: 1,
        },
        PhaseResult {
            id: "fix-implementation".into(),
            outcome: Outcome::GateFailed,
            turns: 9,
            gate_attempts: 5,
        },
    ]);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v["phases"],
        json!([
            {"id": "tdd-reproduction-test", "outcome": "verified", "turns": 3, "gate_attempts": 1},
            {"id": "fix-implementation", "outcome": "gate_failed", "turns": 9, "gate_attempts": 5}
        ])
    );
    let plain =
        serde_json::to_value(RunRecord::from_error(&config(), "x", Duration::ZERO)).unwrap();
    assert_eq!(plain["phases"], json!([]));
}

#[test]
fn tdd_conflicts_with_an_explicit_gate() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["fix it", "--tdd", "--gate", "pytest"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_binary_reports_an_undetectable_repository_as_a_tdd_error() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("run.json");
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["fix it", "--tdd", "--report"])
        .arg(&report)
        .current_dir(dir.path())
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report must exist"))
            .unwrap();
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["gate_source"], "tdd");
    assert!(v["error"].as_str().unwrap().contains("autonomous TDD"));
}
