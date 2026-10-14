use blyte_cli::cli::{apply_orchestrate_defaults, Cli};
use blyte_cli::repl::{
    format_usage_line, handle_repl_command, ReplConfig, ReplControl, ReplSession,
};
use blyte_cli::skill::SKILL_MD;
use blyte_provider::Usage;
use clap::{CommandFactory, FromArgMatches};

#[test]
fn orchestrate_sets_defaults() {
    let matches = Cli::command()
        .try_get_matches_from(["blyte", "--orchestrate"])
        .unwrap();
    let mut cli = Cli::from_arg_matches(&matches).unwrap();
    apply_orchestrate_defaults(&mut cli, &matches);
    assert_eq!(cli.max_turns, 1000);
    assert_eq!(cli.fold_budget, Some(100_000));
    assert_eq!(cli.context, "folded");
}

#[test]
fn explicit_flags_win() {
    let matches = Cli::command()
        .try_get_matches_from([
            "blyte",
            "--orchestrate",
            "--max-turns",
            "30",
            "--fold-budget",
            "5000",
        ])
        .unwrap();
    let mut cli = Cli::from_arg_matches(&matches).unwrap();
    apply_orchestrate_defaults(&mut cli, &matches);
    assert_eq!(cli.max_turns, 30);
    assert_eq!(cli.fold_budget, Some(5000));
}

#[test]
fn without_orchestrate_nothing_changes() {
    let matches = Cli::command().try_get_matches_from(["blyte"]).unwrap();
    let mut cli = Cli::from_arg_matches(&matches).unwrap();
    apply_orchestrate_defaults(&mut cli, &matches);
    assert_eq!(cli.max_turns, 25);
    assert_eq!(cli.fold_budget, None);
}

#[test]
fn orchestrate_conflicts_with_gate() {
    let res = Cli::command().try_get_matches_from(["blyte", "--orchestrate", "--gate", "true"]);
    assert!(res.is_err());
}

#[test]
fn orchestrate_with_prompt_is_accepted() {
    let matches = Cli::command()
        .try_get_matches_from(["blyte", "--orchestrate", "--", "plan the release"])
        .unwrap();
    let cli = Cli::from_arg_matches(&matches).unwrap();
    assert!(cli.orchestrate);
    assert_eq!(cli.prompt.as_deref(), Some("plan the release"));
}

#[test]
fn dispatch_cmd_without_orchestrate_is_rejected() {
    let res = Cli::command().try_get_matches_from(["blyte", "--dispatch-cmd", "echo test"]);
    assert!(res.is_err());
}

#[test]
fn dispatch_cmd_with_orchestrate_is_accepted() {
    let matches = Cli::command()
        .try_get_matches_from([
            "blyte",
            "--orchestrate",
            "--dispatch-cmd",
            "echo test",
            "--",
            "plan the release",
        ])
        .unwrap();
    let cli = Cli::from_arg_matches(&matches).unwrap();
    assert!(cli.orchestrate);
    assert_eq!(cli.dispatch_cmd.as_deref(), Some("echo test"));
    assert_eq!(cli.prompt.as_deref(), Some("plan the release"));
}

#[test]
fn session_gets_instructions() {
    let config = ReplConfig {
        orchestrate: true,
        ..Default::default()
    };
    let session = ReplSession::new(&config);
    let instructions = session.state.orchestrator.instructions.as_deref().unwrap();
    assert!(instructions.contains("## The blyte-implementer skill"));
    assert!(instructions.contains("blyte wait"));

    let config_normal = ReplConfig {
        orchestrate: false,
        ..Default::default()
    };
    let session_normal = ReplSession::new(&config_normal);
    assert!(session_normal.state.orchestrator.instructions.is_none());
}

#[test]
fn gate_refused_in_orchestrate() {
    let config = ReplConfig {
        orchestrate: true,
        ..Default::default()
    };
    let mut session = ReplSession::new(&config);
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/gate true", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("No gate in orchestrate mode."));
    assert_eq!(session.gate, None);
}

#[test]
fn usage_line_format() {
    let line = format_usage_line(
        3,
        &Usage {
            input: 12000,
            cached: 9000,
            output: 450,
            thinking: 0,
        },
    );
    assert_eq!(line, "3 turns · 12000 in (9000 cached) · 450 out");
}

#[test]
fn skill_tells_orchestrator_to_redirect_and_wait() {
    assert!(SKILL_MD.contains("Under `blyte --orchestrate`"));
    assert!(SKILL_MD.contains("> /dev/null 2>&1 &"));
    assert!(SKILL_MD.contains("blyte wait --any"));
}
