//! The `--report` JSON contract that superstate-eval reads. Schema:
//! docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use superstate_agent::run::{Outcome, RunReport};
use superstate_agent::state::{ActionEntry, State};
use superstate_cli::report::{count_insists, count_interceptions, RunRecord};
use superstate_cli::task_runner::TaskConfig;
use superstate_provider::Usage;

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "fix it".into(),
        gate: None,
        context_strategy: "state-only".into(),
        model: Some("gemini-3.8-flash-high".into()),
        container: Some("abc123".into()),
        max_turns: 1000,
        max_gates: 5,
        rollback: true,
        root: PathBuf::from("."),
        loop_breaker: false,
        container_workdir: "/app".into(),
        max_seconds: Some(600),
        report: None,
        tdd: false,
        review: false,
        fold_notes: false,
        max_cost_usd: None,
        journal: None,
        resume: None,
        fold_budget: None,
    }
}

fn action(tool: &str, target: &str, summary: &str) -> ActionEntry {
    ActionEntry {
        turn: 1,
        tool: tool.into(),
        target: target.into(),
        summary: summary.into(),
        is_error: false,
    }
}

#[test]
fn interceptions_are_counted_by_kind() {
    let actions = vec![
        action("read", "a.py", "intercepted: already in working set"),
        action("read", "b.py", "intercepted: already in working set"),
        action("bash", "git status", "intercepted: redundant inspection"),
        action("read", "a.py", "2 lines read"),
    ];
    let i = count_interceptions(&actions);
    assert_eq!((i.read, i.bash), (2, 1));
}

#[test]
fn an_insist_is_a_served_repeat_of_the_intercepted_call() {
    let actions = vec![
        action("read", "a.py", "2 lines read"),
        action("read", "a.py", "intercepted: already in working set"),
        action("read", "a.py", "2 lines read"),
        action("bash", "git status", "intercepted: redundant inspection"),
        action("bash", "git diff", "exit 0"),
        action("read", "b.py", "intercepted: already in working set"),
        action("read", "b.py", "intercepted: already in working set"),
    ];
    assert_eq!(count_insists(&actions), 1);
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
    assert_eq!(v["context"], "state-only");
    assert_eq!(v["loop_breaker"], false);
    assert_eq!(v["gated"], false);
    assert_eq!(v["container"], "abc123");
    assert_eq!(v["container_workdir"], "/app");
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
    assert_eq!(v["interceptions"], json!({"read": 0, "bash": 0}));
    assert_eq!(v["insists"], 0);
    assert_eq!(v["agent_note_keys"], json!([]));
}

#[test]
fn a_completed_run_maps_every_field() {
    let mut state = State::new("fix it", "");
    state
        .agent
        .fields
        .insert("next_step".into(), json!("run tests"));
    state
        .agent
        .fields
        .insert("hypothesis".into(), json!("off by one"));
    state.runtime.rollbacks = 2;
    state.runtime.actions = vec![
        action("read", "a.py", "2 lines read"),
        action("read", "a.py", "intercepted: already in working set"),
        action("read", "a.py", "2 lines read"),
    ];
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
        reviews: 0,
        review_rejections: 0,
        fold_notes: 0,
        fold_note_failures: 0,
        turn_usage: Vec::new(),
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
    assert_eq!(v["interceptions"], json!({"read": 1, "bash": 0}));
    assert_eq!(v["insists"], 1);
    assert_eq!(v["agent_note_keys"], json!(["hypothesis", "next_step"]));
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
    let out = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args([
            "fix it",
            "--model",
            "gemini-3.8-flash-high",
            "--context",
            "state-only",
            "--no-loop-breaker",
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
    assert_eq!(v["context"], "state-only");
    assert_eq!(v["loop_breaker"], false);
    assert_eq!(v["gated"], false);
    assert!(v["container"].is_null());
    assert_eq!(v["container_workdir"], "/app");
    assert_eq!(v["turns"], 0);
}
