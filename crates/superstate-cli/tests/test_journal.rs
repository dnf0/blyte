//! A saved run can be reloaded; the CLI exposes the cost cap, journal and resume.

use clap::Parser;
use superstate_agent::state::State;
use superstate_cli::cli::Cli;
use superstate_cli::journal;
use superstate_provider::Message;

#[test]
fn a_journal_round_trips_and_is_written_atomically() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("run.journal.json");
    let mut state = State::new("fix it", "cargo test");
    state
        .runtime
        .fold_notes
        .insert(4, "Established:\n- x\n".into());
    let history = vec![
        Message::User("Begin. fix it".into()),
        Message::Assistant("looking".into()),
    ];
    journal::write(&path, &state, &history).unwrap();
    journal::write(&path, &state, &history).unwrap();
    let j = journal::read(&path).unwrap();
    assert_eq!(j.schema_version, journal::SCHEMA_VERSION);
    assert_eq!(j.state, state);
    assert_eq!(j.history, history);
    let leftovers: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
}

#[test]
fn reading_a_missing_journal_is_an_error() {
    assert!(journal::read(std::path::Path::new("/nonexistent/j.json")).is_err());
}

#[test]
fn the_cli_parses_cost_cap_journal_and_resume() {
    let cli = Cli::try_parse_from([
        "superstate",
        "--max-cost-usd",
        "2.5",
        "--journal",
        "j.json",
        "--resume",
        "old.json",
    ])
    .unwrap();
    assert_eq!(cli.max_cost_usd, Some(2.5));
    assert_eq!(cli.journal.as_deref(), Some(std::path::Path::new("j.json")));
    assert_eq!(
        cli.resume.as_deref(),
        Some(std::path::Path::new("old.json"))
    );
    assert!(Cli::try_parse_from(["superstate", "--resume", "old.json", "--tdd"]).is_err());
}
