//! The fold budget is settable and recorded; per-turn usage reaches the run report.

use blyte_agent::run::{Outcome, RunReport, TurnUsage};
use blyte_agent::state::State;
use blyte_cli::cli::Cli;
use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;
use clap::Parser;

#[test]
fn the_fold_budget_and_turn_usage_reach_the_report() {
    let cli = Cli::try_parse_from([
        "blyte",
        "--context",
        "folded",
        "--fold-budget",
        "12000",
        "fix it",
    ])
    .unwrap();
    assert_eq!(cli.fold_budget, Some(12000));
    let config = TaskConfig::from(&cli);
    assert_eq!(config.fold_budget, Some(12000));

    let report = RunReport {
        turn_usage: vec![TurnUsage {
            turn: 1,
            input: 10,
            cached: 5,
            output: 1,
            thinking: 0,
            prompt_messages: 1,
            appended: false,
            new_chars: 40,
            fold_budget: None,
        }],
        ..Default::default()
    };
    let r = RunRecord::from_run(
        &config,
        "gemini/gemini-3.8-flash",
        &Outcome::Verified,
        &report,
        &State::new("fix it", ""),
        std::time::Duration::from_secs(1),
    );
    assert_eq!(r.fold_budget, Some(12000));
    assert_eq!(r.turn_usage, report.turn_usage);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["fold_budget"], 12000);
    assert_eq!(v["turn_usage"][0]["new_chars"], 40);
    assert_eq!(v["turn_usage"][0]["appended"], false);
}

#[test]
fn the_fold_budget_defaults_to_unset() {
    let cli = Cli::try_parse_from(["blyte", "fix it"]).unwrap();
    assert_eq!(TaskConfig::from(&cli).fold_budget, None);
}
