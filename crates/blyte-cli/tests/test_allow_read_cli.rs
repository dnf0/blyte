use std::path::PathBuf;

use clap::Parser;

use blyte_cli::cli::Cli;
use blyte_cli::task_runner::TaskConfig;

#[test]
fn cli_allow_read_flags_are_parsed_and_carried_to_task_config() {
    let cli = Cli::try_parse_from(["blyte", "--allow-read", "/a", "--allow-read", "/b"]).unwrap();
    assert_eq!(
        cli.allow_read,
        vec![PathBuf::from("/a"), PathBuf::from("/b")]
    );
    let config = TaskConfig::from(&cli);
    assert_eq!(
        config.allow_read,
        vec![PathBuf::from("/a"), PathBuf::from("/b")]
    );
}
