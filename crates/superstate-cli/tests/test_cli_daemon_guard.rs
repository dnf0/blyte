use clap::Parser;
use superstate_cli::state_cli::{Commands, StateArgs, Subcommands};

#[test]
fn test_cli_no_guard_flag_parsing() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--no-guard"])
        .expect("Failed to parse --no-guard flag");
    match args.command {
        Subcommands::Daemon(daemon_args) => {
            assert!(
                daemon_args.no_guard,
                "Expected no_guard to be true when --no-guard is passed"
            );
            assert!(
                !daemon_args.use_guard(),
                "Expected use_guard() to return false when --no-guard is passed"
            );
        }
        _ => panic!("Expected Subcommands::Daemon"),
    }
}

#[test]
fn test_cli_no_guard_flag_default_false() {
    let args = StateArgs::try_parse_from(["state", "daemon"]).expect("Failed to parse daemon args");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                !daemon_args.no_guard,
                "Expected no_guard to default to false"
            );
            assert!(
                daemon_args.use_guard(),
                "Expected use_guard() to default to true"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}
