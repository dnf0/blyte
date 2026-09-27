//! The report records the run's security settings, so a run can be audited afterwards: `sandboxed`
//! alone could not tell Seatbelt from Landlock or show an `--allow-read` grant (2026-09-27).

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use blyte_cli::cli::Cli;
use blyte_cli::report::{sandbox_kind, RunRecord};
use blyte_cli::task_runner::TaskConfig;

fn config(args: &[&str]) -> TaskConfig {
    let mut argv = vec!["blyte"];
    argv.extend_from_slice(args);
    TaskConfig::from(&Cli::try_parse_from(argv).unwrap())
}

#[test]
fn grants_and_modes_are_recorded() {
    let c = config(&[
        "--allow-read",
        "/a",
        "--allow-read",
        "/b",
        "--pass-env",
        "GH_TOKEN",
        "--network",
        "off",
    ]);
    let r = serde_json::to_value(RunRecord::from_error(&c, "x", Duration::from_secs(1))).unwrap();
    assert_eq!(r["allow_read"], serde_json::json!(["/a", "/b"]));
    assert_eq!(r["pass_env"], serde_json::json!(["GH_TOKEN"]));
    assert_eq!(r["network"], "off");
    assert!(r["withheld_env"].is_array(), "{r}");
    assert!(r["sandbox"].is_string(), "{r}");
}

#[test]
fn the_sandbox_kind_names_the_mechanism() {
    let mut c = config(&[]);
    c.root = PathBuf::from(".");
    let kind = sandbox_kind(&c);
    if !blyte_agent::sandbox::available() {
        assert_eq!(kind, "off");
    } else if cfg!(target_os = "macos") {
        assert_eq!(kind, "seatbelt");
    } else {
        assert_eq!(kind, "landlock");
    }
    let off = config(&["--sandbox", "off"]);
    assert_eq!(sandbox_kind(&off), "off");
    let ctr = config(&["--container", "abc"]);
    assert_eq!(sandbox_kind(&ctr), "container");
}

#[test]
fn a_container_run_withholds_nothing_because_it_inherits_nothing() {
    std::env::set_var("ZZ_REPORT_TEST_TOKEN", "v");
    let c = config(&["--container", "abc"]);
    let r = serde_json::to_value(RunRecord::from_error(&c, "x", Duration::from_secs(1))).unwrap();
    assert_eq!(r["withheld_env"], serde_json::json!([]));
    let local = serde_json::to_value(RunRecord::from_error(
        &config(&[]),
        "x",
        Duration::from_secs(1),
    ))
    .unwrap();
    assert!(
        local["withheld_env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "ZZ_REPORT_TEST_TOKEN"),
        "{local}"
    );
}
