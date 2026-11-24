//! The run record carries the failed gate output.

use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport};
use blyte_agent::state::State;
use blyte_cli::dispatch::cut_summary_gate_failure;
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
        network: "on".into(),
        allow_read: Vec::new(),
        pass_env: Vec::new(),
        research: false,
        claim_check: false,
        base_check: false,
        base_check_dir: None,
        converse: false,
        dispatch_cmd: None,
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

#[test]
fn a_long_check_failure_keeps_its_end_in_the_summary() {
    let first_line = "1 of 1 steps failed: pytest";
    let last_line = "FAILED tests/x.py::test_y";
    let mut entry = String::from(first_line);
    entry.push('\n');
    let mut line_no = 1usize;
    while entry.chars().count() + last_line.chars().count() + 40 < 4_000 {
        entry.push_str(&format!(
            "collecting item {line_no:04} ..................\n"
        ));
        line_no += 1;
    }
    let pad = 4_000usize.saturating_sub(entry.chars().count() + last_line.chars().count() + 1);
    for _ in 0..pad {
        entry.push('x');
    }
    entry.push('\n');
    entry.push_str(last_line);
    assert_eq!(entry.chars().count(), 4_000);

    let cut = cut_summary_gate_failure(&entry);
    let lines: Vec<&str> = cut.lines().collect();
    assert_eq!(lines.first().copied(), Some(first_line));
    assert_eq!(lines.last().copied(), Some(last_line));

    let marker = lines[1];
    assert!(marker.starts_with("[... "), "{marker}");
    assert!(marker.ends_with(" characters left out ...]"), "{marker}");
    let omitted: usize = marker
        .trim_start_matches("[... ")
        .trim_end_matches(" characters left out ...]")
        .parse()
        .unwrap();

    let kept_head = lines[0].chars().count();
    let kept_tail = cut
        .split_once(&format!("\n{marker}\n"))
        .unwrap()
        .1
        .chars()
        .count();
    assert!(
        kept_head + kept_tail <= 1_000,
        "{} + {}",
        kept_head,
        kept_tail
    );
    assert_eq!(omitted, 4_000 - kept_head - kept_tail);
    assert!(
        cut.chars().count() <= 1_000 + marker.chars().count() + 2,
        "{}",
        cut.chars().count()
    );
}

#[test]
fn a_short_check_failure_is_kept_whole() {
    let short = "exit 1\nFAILED tests/x.py::test_y";
    assert_eq!(cut_summary_gate_failure(short), short);

    let exact_limit = "a".repeat(1_000);
    assert_eq!(cut_summary_gate_failure(&exact_limit), exact_limit);
}

#[test]
fn a_cut_never_splits_a_character() {
    let entry = "é".repeat(3_000);
    let cut = cut_summary_gate_failure(&entry);
    assert!(cut.starts_with(&"é".repeat(200)));
    assert!(cut.contains("[... 2000 characters left out ...]"));
    assert!(cut.ends_with(&"é".repeat(800)));
}
