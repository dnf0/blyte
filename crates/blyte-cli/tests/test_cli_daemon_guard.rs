use blyte_cli::state_cli::{Commands, StateArgs};
use clap::Parser;

#[test]
fn test_cli_no_guard_flag_parsing() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--no-guard"])
        .expect("Failed to parse --no-guard flag");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                daemon_args.no_guard,
                "Expected no_guard to be true when --no-guard is passed"
            );
            assert!(
                !daemon_args.use_guard(),
                "Expected use_guard() to return false when --no-guard is passed"
            );
            assert!(
                !daemon_args.use_command_guard(),
                "Expected use_command_guard() to return false when --no-guard is passed"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
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
            assert!(
                daemon_args.use_command_guard(),
                "Expected use_command_guard() to default to true"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}

#[test]
fn test_cli_no_command_guard_flag_parsing() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--no-command-guard"])
        .expect("Failed to parse --no-command-guard flag");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                daemon_args.no_command_guard,
                "Expected no_command_guard to be true when --no-command-guard is passed"
            );
            assert!(
                !daemon_args.use_command_guard(),
                "Expected use_command_guard() to return false when --no-command-guard is passed"
            );
            assert!(
                daemon_args.use_guard(),
                "Expected use_guard() to return true when only --no-command-guard is passed"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}

#[test]
fn test_cli_no_command_guard_default_false() {
    let args = StateArgs::try_parse_from(["state", "daemon"]).expect("Failed to parse daemon args");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                !daemon_args.no_command_guard,
                "Expected no_command_guard to default to false"
            );
            assert!(
                daemon_args.use_command_guard(),
                "Expected use_command_guard() to default to true"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}

#[test]
fn test_cli_no_file_guard_flag_parsing() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--no-file-guard"])
        .expect("Failed to parse --no-file-guard flag");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                daemon_args.no_file_guard,
                "Expected no_file_guard to be true when --no-file-guard is passed"
            );
            assert!(
                !daemon_args.use_guard(),
                "Expected use_guard() to return false when --no-file-guard is passed"
            );
            assert!(
                daemon_args.use_command_guard(),
                "Expected use_command_guard() to return true when only --no-file-guard is passed"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}

#[test]
fn test_cli_no_guard_disables_both() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--no-guard"])
        .expect("Failed to parse --no-guard flag");
    match args.command {
        Commands::Daemon(daemon_args) => {
            assert!(
                daemon_args.no_guard,
                "Expected no_guard to be true when --no-guard is passed"
            );
            assert!(
                !daemon_args.use_guard(),
                "Expected use_guard() to return false when --no-guard is passed"
            );
            assert!(
                !daemon_args.use_command_guard(),
                "Expected use_command_guard() to return false when --no-guard is passed"
            );
        }
        _ => panic!("Expected Commands::Daemon"),
    }
}
