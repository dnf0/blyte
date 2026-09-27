//! The `--report` JSON contract that superstate-eval reads. Schema:
//! docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport};
use blyte_agent::state::State;
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;
use blyte_provider::Usage;
use serde_json::json;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "fix it".into(),
        gate: None,
        context_strategy: "folded".into(),
        model: Some("gemini-3.8-flash-high".into()),
        container: Some("abc123".into()),
        max_turns: 1000,
        max_gates: 5,
        rollback: true,
        root: PathBuf::from("."),
        container_workdir: "/app".into(),
        max_seconds: Some(600),
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        allow_read: Vec::new(),
    }
}

#[test]
fn an_error_record_has_zeroed_counters_and_no_model_id() {
    let r = RunRecord::from_error(
        &config(),
        "Provider error: no API key",
        Duration::from_millis(1500),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["error"], "Provider error: no API key");
    assert_eq!(v["model_requested"], "gemini-3.8-flash-high");
    assert!(v["model_id"].is_null());
    assert_eq!(v["context"], "folded");
    assert_eq!(v["gated"], false);
    assert_eq!(v["container"], "abc123");
    assert_eq!(v["container_workdir"], "/app");
    assert_eq!(v["sandboxed"], false);
    assert_eq!(v["duration_seconds"], 1.5);
    assert_eq!(v["turns"], 0);
    assert_eq!(v["empty_turns"], 0);
    assert_eq!(v["tool_calls"], 0);
    assert_eq!(v["gate_attempts"], 0);
    assert_eq!(v["rollbacks"], 0);
    assert_eq!(
        v["usage"],
        json!({"input": 0, "cached": 0, "output": 0, "thinking": 0})
    );
}

#[test]
fn a_completed_run_maps_every_field() {
    let mut state = State::new("fix it", "");
    state.runtime.rollbacks = 2;
    let report = RunReport {
        outcome_turns: 7,
        usage: Usage {
            input: 100,
            cached: 50,
            output: 20,
            thinking: 5,
        },
        empty_turns: 1,
        tool_calls: 3,
        gate_attempts: 2,
        provider_retries: 0,
        turn_usage: Vec::new(),
        ..Default::default()
    };
    let r = RunRecord::from_run(
        &config(),
        "gemini/gemini-3.8-flash",
        &Outcome::BudgetExhausted,
        &report,
        &state,
        Duration::from_secs(3),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["outcome"], "budget_exhausted");
    assert!(v["error"].is_null());
    assert_eq!(v["model_id"], "gemini/gemini-3.8-flash");
    assert_eq!(v["turns"], 7);
    assert_eq!(v["empty_turns"], 1);
    assert_eq!(v["tool_calls"], 3);
    assert_eq!(v["gate_attempts"], 2);
    assert_eq!(v["rollbacks"], 2);
    assert_eq!(
        v["usage"],
        json!({"input": 100, "cached": 50, "output": 20, "thinking": 5})
    );
}

#[test]
fn an_error_outcome_carries_its_message() {
    let state = State::new("fix it", "");
    let r = RunRecord::from_run(
        &config(),
        "gemini/gemini-3.8-flash",
        &Outcome::Error("provider returned 500".into()),
        &RunReport::default(),
        &state,
        Duration::from_secs(1),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["error"], "provider returned 500");
}

#[test]
fn gated_is_true_only_for_a_non_empty_gate() {
    let mut blank = config();
    blank.gate = Some("   ".into());
    assert!(!RunRecord::from_error(&blank, "x", Duration::ZERO).gated);
    let mut real = config();
    real.gate = Some("pytest".into());
    assert!(RunRecord::from_error(&real, "x", Duration::ZERO).gated);
}

#[test]
fn the_binary_writes_a_report_when_the_provider_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("run.json");
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "fix it",
            "--model",
            "gemini-3.8-flash-high",
            "--context",
            "folded",
            "--container-workdir",
            "/app",
            "--max-seconds",
            "600",
            "--report",
        ])
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
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["outcome"], "error");
    assert!(v["error"].as_str().unwrap().contains("GEMINI_API_KEY"));
    assert_eq!(v["model_requested"], "gemini-3.8-flash-high");
    assert!(v["model_id"].is_null());
    assert_eq!(v["context"], "folded");
    assert_eq!(v["gated"], false);
    assert!(v["container"].is_null());
    assert_eq!(v["container_workdir"], "/app");
    assert_eq!(v["sandboxed"], blyte_agent::sandbox::available());
    assert_eq!(v["turns"], 0);
}

#[test]
fn sandboxed_reflects_configuration_and_availability() {
    let mut local_on = config();
    local_on.container = None;
    local_on.sandbox = "on".into();
    assert_eq!(
        RunRecord::from_error(&local_on, "err", Duration::ZERO).sandboxed,
        blyte_agent::sandbox::available()
    );

    let mut local_off = config();
    local_off.container = None;
    local_off.sandbox = "off".into();
    assert!(!RunRecord::from_error(&local_off, "err", Duration::ZERO).sandboxed);

    let mut container_on = config();
    container_on.container = Some("cid".into());
    container_on.sandbox = "on".into();
    assert!(!RunRecord::from_error(&container_on, "err", Duration::ZERO).sandboxed);
}
