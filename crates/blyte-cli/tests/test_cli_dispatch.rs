use blyte_cli::cli::{Cli, Commands};
use blyte_cli::state_cli::{InitArgs, StateCommands};
use clap::Parser;
use std::path::PathBuf;

#[test]
fn test_parse_no_args_launches_interactive() {
    let cli = Cli::try_parse_from(["blyte"]).expect("should parse");
    assert_eq!(cli.prompt, None);
    assert_eq!(cli.command, None);
    assert_eq!(cli.context, "folded");
    assert_eq!(cli.max_turns, 25);
    assert_eq!(cli.max_gates, 5);
    assert!(cli.rollback);
    assert_eq!(cli.model, None);
    assert_eq!(cli.container, None);
}

#[test]
fn test_parse_single_shot_prompt() {
    let cli = Cli::try_parse_from([
        "blyte",
        "fix issue",
        "--gate",
        "pytest",
        "--context",
        "state-only",
    ])
    .expect("should parse");
    assert_eq!(cli.prompt, Some("fix issue".to_string()));
    assert_eq!(cli.gate, Some("pytest".to_string()));
    assert_eq!(cli.context, "state-only");
    assert_eq!(cli.command, None);
}

#[test]
fn test_parse_custom_execution_flags() {
    let cli = Cli::try_parse_from([
        "blyte",
        "solve bug",
        "--model",
        "gemini-2.5-pro",
        "--container",
        "docker-workspace",
        "--max-turns",
        "42",
        "--max-gates",
        "10",
        "--rollback",
    ])
    .expect("should parse");
    assert_eq!(cli.prompt.as_deref(), Some("solve bug"));
    assert_eq!(cli.model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(cli.container.as_deref(), Some("docker-workspace"));
    assert_eq!(cli.max_turns, 42);
    assert_eq!(cli.max_gates, 10);
    assert!(cli.rollback);
}

#[test]
fn test_parse_rollback_disabled() {
    let cli_eq = Cli::try_parse_from(["blyte", "task", "--rollback=false"]).expect("should parse");
    assert!(!cli_eq.rollback);

    let cli_preceding =
        Cli::try_parse_from(["blyte", "--rollback=false", "task"]).expect("should parse");
    assert!(!cli_preceding.rollback);
    assert_eq!(cli_preceding.prompt.as_deref(), Some("task"));

    let cli_true = Cli::try_parse_from(["blyte", "--rollback=true", "task"]).expect("should parse");
    assert!(cli_true.rollback);
    assert_eq!(cli_true.prompt.as_deref(), Some("task"));

    let cli_flag_preceding =
        Cli::try_parse_from(["blyte", "--rollback", "task"]).expect("should parse");
    assert!(cli_flag_preceding.rollback);
    assert_eq!(cli_flag_preceding.prompt.as_deref(), Some("task"));
}

#[test]
fn test_context_validation() {
    let cli =
        Cli::try_parse_from(["blyte", "task", "--context", "state-only"]).expect("should parse");
    assert_eq!(cli.context, "state-only");

    let invalid = Cli::try_parse_from(["blyte", "task", "--context", "invalid-strategy"]);
    assert!(
        invalid.is_err(),
        "invalid context strategy must be rejected"
    );
}

#[test]
fn test_parse_subcommand_doctor() {
    let cli = Cli::try_parse_from(["blyte", "doctor"]).expect("should parse");
    assert_eq!(cli.prompt, None);
    assert_eq!(cli.command, Some(Commands::Doctor));
}

#[test]
fn test_parse_subcommand_mcp() {
    let cli = Cli::try_parse_from(["blyte", "mcp"]).expect("should parse");
    assert_eq!(cli.prompt, None);
    assert_eq!(cli.command, Some(Commands::Mcp));
}

#[test]
fn test_parse_subcommand_state_init() {
    let cli =
        Cli::try_parse_from(["blyte", "state", "init", "--plan", "plan.md"]).expect("should parse");
    assert_eq!(cli.prompt, None);
    let expected = Commands::State {
        cmd: StateCommands::Init(InitArgs {
            plan: Some(PathBuf::from("plan.md")),
            state: PathBuf::from(".agent-state/state.json"),
            worktree: false,
            no_worktree: false,
            goal: None,
            verify_cmd: None,
            files: None,
            milestones: None,
        }),
    };
    assert_eq!(cli.command, Some(expected));
}

#[test]
fn test_prompt_conflicts_with_subcommand() {
    let result = Cli::try_parse_from(["blyte", "fix task", "doctor"]);
    assert!(result.is_err(), "prompt should conflict with subcommand");
}
