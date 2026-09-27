use clap::Parser;

use blyte_cli::cli::Cli;
use blyte_cli::task_runner::TaskConfig;

#[test]
fn cli_pass_env_flags_are_parsed_and_carried_to_task_config() {
    let cli = Cli::try_parse_from(["blyte", "--pass-env", "A", "--pass-env", "B"]).unwrap();
    assert_eq!(cli.pass_env, vec!["A".to_string(), "B".to_string()]);
    let config = TaskConfig::from(&cli);
    assert_eq!(config.pass_env, vec!["A".to_string(), "B".to_string()]);
}
