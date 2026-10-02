//! The run report carries the timing and per-tool counts, with totals in seconds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport, TurnUsage};
use blyte_agent::state::State;
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "p".into(),
        gate: Some("true".into()),
        context_strategy: "folded".into(),
        model: None,
        container: None,
        max_turns: 10,
        max_gates: 5,
        rollback: false,
        root: PathBuf::from("."),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
    }
}

#[test]
fn the_record_carries_timing_and_tool_counts() {
    let report = RunReport {
        turn_usage: vec![
            TurnUsage {
                turn: 1,
                model_ms: 1500,
                tools_ms: 250,
                ..Default::default()
            },
            TurnUsage {
                turn: 2,
                model_ms: 500,
                tools_ms: 0,
                ..Default::default()
            },
        ],
        gate_attempts: 2,
        gate_ms: vec![3000, 4500],
        tool_counts: BTreeMap::from([("bash".to_string(), 2), ("read".to_string(), 1)]),
        ..Default::default()
    };
    let r = RunRecord::from_run(
        &config(),
        "scripted",
        &Outcome::Verified,
        &report,
        &State::new("p", "true"),
        Duration::from_secs(10),
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["gate_ms"], serde_json::json!([3000, 4500]));
    assert_eq!(v["tool_counts"]["bash"], 2);
    assert_eq!(v["model_seconds"], 2.0);
    assert_eq!(v["tools_seconds"], 0.25);
    assert_eq!(v["gate_seconds"], 7.5);
    assert_eq!(v["turn_usage"][0]["model_ms"], 1500);
}

#[test]
fn an_error_record_has_zero_timing() {
    let v = serde_json::to_value(RunRecord::from_error(&config(), "e", Duration::ZERO)).unwrap();
    assert_eq!(v["gate_ms"], serde_json::json!([]));
    assert_eq!(v["model_seconds"], 0.0);
}
