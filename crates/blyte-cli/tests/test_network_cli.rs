use clap::Parser;

use blyte_cli::cli::Cli;
use blyte_cli::task_runner::{check_network_mode, run_task, TaskConfig};

#[test]
fn cli_network_flag_defaults_to_on() {
    let cli = Cli::try_parse_from(["blyte"]).unwrap();
    assert_eq!(cli.network, "on");
    let config = TaskConfig::from(&cli);
    assert_eq!(config.network, "on");
}

#[test]
fn cli_network_flag_parses_off() {
    let cli = Cli::try_parse_from(["blyte", "--network", "off"]).unwrap();
    assert_eq!(cli.network, "off");
    let config = TaskConfig::from(&cli);
    assert_eq!(config.network, "off");
}

#[test]
fn cli_network_flag_rejects_invalid_value() {
    let res = Cli::try_parse_from(["blyte", "--network", "maybe"]);
    assert!(res.is_err());
}

#[test]
fn network_off_with_sandbox_off_is_refused() {
    let cli = Cli::try_parse_from(["blyte", "--network", "off", "--sandbox", "off"]).unwrap();
    let config = TaskConfig::from(&cli);
    let check = check_network_mode(&config);
    assert!(check.is_err());
    assert!(check.unwrap_err().contains("needs the sandbox"));

    let run_res = run_task(config);
    assert!(run_res.is_err());
    assert!(run_res.unwrap_err().contains("needs the sandbox"));
}

#[test]
fn network_off_with_container_is_refused() {
    let cli =
        Cli::try_parse_from(["blyte", "--network", "off", "--container", "my-container"]).unwrap();
    let config = TaskConfig::from(&cli);
    let check = check_network_mode(&config);
    assert!(check.is_err());
    assert!(check
        .unwrap_err()
        .contains("set the container's network instead"));
}
