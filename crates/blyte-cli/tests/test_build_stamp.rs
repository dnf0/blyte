//! The binary knows the commit it was built from: the ledger used to record the source
//! checkout's HEAD, which drifts from the installed binary.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;

fn head() -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "p".into(),
        gate: None,
        context_strategy: "folded".into(),
        model: None,
        container: None,
        max_turns: 1,
        max_gates: 1,
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
fn a_report_carries_the_commit_the_binary_was_built_from() {
    let r = RunRecord::from_error(&config(), "e", Duration::from_secs(0));
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["harness_build"], head());
}

#[test]
fn version_prints_the_build_commit() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("--version")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("({})", head())), "{stdout}");
}
