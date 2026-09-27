//! The run record carries the failed gate output.

use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport};
use blyte_agent::state::State;
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "p".into(),
        gate: Some("false".into()),
        context_strategy: "folded".into(),
        model: None,
        container: None,
        max_turns: 5,
        max_gates: 2,
        rollback: false,
        root: PathBuf::from("."),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        max_cost_usd: None,
        report: None,
        fold_budget: None,
        sandbox: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
    }
}

#[test]
fn the_record_carries_gate_failures() {
    let report = RunReport {
        gate_failures: vec!["exit 1\nboom".into()],
        ..Default::default()
    };
    let r = RunRecord::from_run(
        &config(),
        "m",
        &Outcome::GateFailed,
        &report,
        &State::new("p", "false"),
        Duration::ZERO,
    );
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["gate_failures"][0], "exit 1\nboom");
    let e = serde_json::to_value(RunRecord::from_error(&config(), "e", Duration::ZERO)).unwrap();
    assert_eq!(e["gate_failures"], serde_json::json!([]));
}
