use blyte_cli::state_cli::{Commands, StateArgs, Subcommands};
use clap::Parser;

#[test]
fn test_cli_impact_slicing_flag_parsing() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--impact-slicing"])
        .expect("Failed to parse --impact-slicing flag");
    match args.command {
        Subcommands::Daemon(daemon_args) => {
            assert!(
                daemon_args.impact_slicing,
                "Expected impact_slicing to be true"
            );
        }
        _ => panic!("Expected Subcommands::Daemon"),
    }
}

#[test]
fn test_cli_impact_slicing_flag_default_false() {
    let args = StateArgs::try_parse_from(["state", "daemon"]).expect("Failed to parse daemon args");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                !daemon_args.impact_slicing,
                "Expected impact_slicing to default to false"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}
