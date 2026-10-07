//! Tests for `blyte cost` CLI subcommand.

use std::path::Path;
use std::process::Command;

fn blyte_cmd(dir: &Path, args: &[&str], usage_log: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.args(args)
        .current_dir(dir)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL");
    if let Some(log) = usage_log {
        cmd.env("BLYTE_USAGE_LOG", log.as_os_str());
    } else {
        cmd.env_remove("BLYTE_USAGE_LOG");
    }
    cmd.output().unwrap()
}

#[test]
fn cost_with_missing_log_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let missing_path = tmp.path().join("missing.jsonl");

    let out = blyte_cmd(tmp.path(), &["cost"], Some(&missing_path));
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        "blyte $0.00 (orchestrator $0.00, implementers $0.00 in 0 runs)"
    );

    let out_json = blyte_cmd(tmp.path(), &["cost", "--json"], Some(&missing_path));
    assert_eq!(out_json.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out_json.stdout).unwrap();
    assert_eq!(v["orchestrator_usd"], 0.0);
    assert_eq!(v["implementers_usd"], 0.0);
    assert_eq!(v["runs"], 0);
}

#[test]
fn cost_cli_with_entries_and_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("usage.jsonl");

    let entry1 = blyte_cli::usage_log::Entry {
        ts: 1000,
        repo: "/repo/a".into(),
        session: Some("sess-1".into()),
        dispatch: None,
        research: false,
        model: "m1".into(),
        outcome: "verified".into(),
        turns: 1,
        input: 100,
        cached: 0,
        output: 50,
        thinking: 0,
        cost_usd: Some(0.15),
    };
    let entry2 = blyte_cli::usage_log::Entry {
        ts: 2000,
        repo: "/repo/a".into(),
        session: Some("sess-1".into()),
        dispatch: Some("disp-1".into()),
        research: false,
        model: "m2".into(),
        outcome: "verified".into(),
        turns: 2,
        input: 200,
        cached: 0,
        output: 100,
        thinking: 0,
        cost_usd: Some(0.35),
    };
    let entry3 = blyte_cli::usage_log::Entry {
        ts: 3000,
        repo: "/repo/b".into(),
        session: Some("sess-2".into()),
        dispatch: Some("disp-2".into()),
        research: false,
        model: "m3".into(),
        outcome: "verified".into(),
        turns: 3,
        input: 300,
        cached: 0,
        output: 150,
        thinking: 0,
        cost_usd: Some(0.50),
    };

    blyte_cli::usage_log::append(&log_path, &entry1).unwrap();
    blyte_cli::usage_log::append(&log_path, &entry2).unwrap();
    blyte_cli::usage_log::append(&log_path, &entry3).unwrap();

    // All entries
    let out = blyte_cmd(tmp.path(), &["cost"], Some(&log_path));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "blyte $1.00 (orchestrator $0.15, implementers $0.85 in 2 runs)"
    );

    // JSON format
    let out = blyte_cmd(tmp.path(), &["cost", "--json"], Some(&log_path));
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!((v["orchestrator_usd"].as_f64().unwrap() - 0.15).abs() < 1e-4);
    assert!((v["implementers_usd"].as_f64().unwrap() - 0.85).abs() < 1e-4);
    assert_eq!(v["runs"], 2);

    // Filter by session
    let out = blyte_cmd(
        tmp.path(),
        &["cost", "--session", "sess-1"],
        Some(&log_path),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "blyte $0.50 (orchestrator $0.15, implementers $0.35 in 1 runs)"
    );

    // Filter by since
    let out = blyte_cmd(tmp.path(), &["cost", "--since", "2000"], Some(&log_path));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "blyte $0.85 (orchestrator $0.00, implementers $0.85 in 2 runs)"
    );

    // Filter by repo
    let out = blyte_cmd(tmp.path(), &["cost", "--repo", "/repo/b"], Some(&log_path));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "blyte $0.50 (orchestrator $0.00, implementers $0.50 in 1 runs)"
    );

    // Combined filter
    let out = blyte_cmd(
        tmp.path(),
        &[
            "cost",
            "--session",
            "sess-1",
            "--since",
            "1500",
            "--repo",
            "/repo/a",
        ],
        Some(&log_path),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "blyte $0.35 (orchestrator $0.00, implementers $0.35 in 1 runs)"
    );
}
