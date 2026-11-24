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

fn ledger_entry(
    outcome: &str,
    dispatch: Option<&str>,
    cost_usd: Option<f64>,
) -> blyte_cli::usage_log::Entry {
    blyte_cli::usage_log::Entry {
        ts: 1000,
        repo: "/repo/a".into(),
        session: Some("sess-1".into()),
        dispatch: dispatch.map(String::from),
        research: false,
        model: "m1".into(),
        outcome: outcome.into(),
        turns: 1,
        input: 100,
        cached: 0,
        output: 50,
        thinking: 0,
        cost_usd,
    }
}

#[test]
fn turn_lines_do_not_count_as_runs() {
    let entries = vec![
        ledger_entry("turn", Some("d1"), Some(0.10)),
        ledger_entry("turn", Some("d1"), Some(0.20)),
        ledger_entry("turn", Some("d1"), Some(0.30)),
        ledger_entry("verified", Some("d1"), Some(0.05)),
    ];
    let t = blyte_cli::cost::totals(&entries, None, None, None);
    assert_eq!(t.runs, 1);
    assert!((t.implementers_usd - 0.65).abs() < 1e-9);
    assert_eq!(t.orchestrator_usd, 0.0);
}

#[test]
fn old_ledger_totals_unchanged() {
    let entries = vec![
        ledger_entry("verified", Some("d1"), Some(0.25)),
        ledger_entry("budgetexhausted", Some("d2"), Some(0.35)),
    ];
    let t = blyte_cli::cost::totals(&entries, None, None, None);
    assert_eq!(t.runs, 2);
    assert!((t.implementers_usd - 0.60).abs() < 1e-9);
    assert_eq!(t.orchestrator_usd, 0.0);
}

#[test]
fn a_killed_run_keeps_its_cost() {
    let entries = vec![
        ledger_entry("turn", Some("d1"), Some(0.15)),
        ledger_entry("turn", Some("d1"), Some(0.25)),
    ];
    let t = blyte_cli::cost::totals(&entries, None, None, None);
    assert_eq!(t.runs, 0);
    assert!((t.implementers_usd - 0.40).abs() < 1e-9);
    assert_eq!(t.orchestrator_usd, 0.0);
}

#[test]
fn model_review_rows_add_cost_but_not_runs() {
    let entries = vec![
        ledger_entry("turn", Some("d1"), Some(0.10)),
        ledger_entry("verified", Some("d1"), Some(0.05)),
        ledger_entry("model_review", Some("d1"), Some(0.07)),
        ledger_entry("model_review", Some("d1"), Some(0.03)),
    ];
    let t = blyte_cli::cost::totals(&entries, None, None, None);
    assert_eq!(t.runs, 1);
    assert!((t.implementers_usd - 0.25).abs() < 1e-9);
    assert_eq!(t.orchestrator_usd, 0.0);

    let only_review = vec![ledger_entry("model_review", Some("d2"), Some(0.12))];
    let t2 = blyte_cli::cost::totals(&only_review, None, None, None);
    assert_eq!(t2.runs, 0);
    assert!((t2.implementers_usd - 0.12).abs() < 1e-9);
}

#[test]
fn session_filter_selects_only_that_session() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("usage.jsonl");

    let rows = vec![
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_100,
            repo: "/repo/shared".into(),
            session: Some("sess-a".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "turn".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(0.10),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_150,
            repo: "/repo/shared".into(),
            session: Some("sess-b".into()),
            dispatch: Some("d2".into()),
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 2,
            input: 200,
            cached: 0,
            output: 80,
            thinking: 0,
            cost_usd: Some(0.40),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_200,
            repo: "/repo/shared".into(),
            session: Some("sess-a".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 0,
            input: 0,
            cached: 0,
            output: 0,
            thinking: 0,
            cost_usd: Some(0.05),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_250,
            repo: "/repo/shared".into(),
            session: Some("sess-a".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "model_review".into(),
            turns: 1,
            input: 150,
            cached: 0,
            output: 30,
            thinking: 0,
            cost_usd: Some(0.03),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_300,
            repo: "/repo/shared".into(),
            session: Some("sess-b".into()),
            dispatch: None,
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 20,
            thinking: 0,
            cost_usd: Some(0.20),
        },
    ];
    for row in &rows {
        blyte_cli::usage_log::append(&log_path, row).unwrap();
    }

    let out_a = blyte_cmd(
        dir.path(),
        &["cost", "--session", "sess-a", "--repo", "/repo/shared"],
        Some(&log_path),
    );
    assert_eq!(out_a.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out_a.stdout).unwrap().trim(),
        "blyte $0.18 (orchestrator $0.00, implementers $0.18 in 1 runs)"
    );

    let out_b = blyte_cmd(
        dir.path(),
        &["cost", "--session", "sess-b", "--repo", "/repo/shared"],
        Some(&log_path),
    );
    assert_eq!(out_b.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out_b.stdout).unwrap().trim(),
        "blyte $0.60 (orchestrator $0.20, implementers $0.40 in 1 runs)"
    );

    let out_unknown = blyte_cmd(
        dir.path(),
        &["cost", "--session", "does-not-exist"],
        Some(&log_path),
    );
    assert_eq!(out_unknown.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out_unknown.stdout).unwrap().trim(),
        "blyte $0.00 (orchestrator $0.00, implementers $0.00 in 0 runs)"
    );

    let out_help = blyte_cmd(dir.path(), &["cost", "--help"], Some(&log_path));
    assert_eq!(out_help.status.code(), Some(0));
    let help = String::from_utf8(out_help.stdout).unwrap();
    assert!(help.contains("--by-session"), "{help}");
    assert!(help.contains("unknown session"), "{help}");
}

#[test]
fn by_session_lists_each_session_and_the_total() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("usage.jsonl");

    let rows = vec![
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_200,
            repo: "/repo/a".into(),
            session: Some("sess-late".into()),
            dispatch: Some("d2".into()),
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 2,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(0.40),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_100,
            repo: "/repo/a".into(),
            session: Some("sess-early".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "turn".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(0.10),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_120,
            repo: "/repo/a".into(),
            session: Some("sess-early".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "turn".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(0.15),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_140,
            repo: "/repo/a".into(),
            session: Some("sess-early".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 0,
            input: 0,
            cached: 0,
            output: 0,
            thinking: 0,
            cost_usd: Some(0.00),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_300,
            repo: "/repo/a".into(),
            session: Some("sess-early".into()),
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "model_review".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(0.05),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_050,
            repo: "/repo/other".into(),
            session: Some("sess-other-repo".into()),
            dispatch: Some("d9".into()),
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(9.99),
        },
    ];
    for row in &rows {
        blyte_cli::usage_log::append(&log_path, row).unwrap();
    }

    let out_text = blyte_cmd(
        dir.path(),
        &[
            "cost",
            "--by-session",
            "--repo",
            "/repo/a",
            "--since",
            "1700000100",
        ],
        Some(&log_path),
    );
    assert_eq!(out_text.status.code(), Some(0));
    let stdout = String::from_utf8(out_text.stdout).unwrap();
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 3, "stdout:\n{stdout}");
    assert!(lines[0].starts_with("sess-early  "), "{}", lines[0]);
    assert!(
        lines[0].contains("1 runs, 2 turns, 1 model reviews"),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("$0.30"), "{}", lines[0]);
    assert!(lines[1].starts_with("sess-late  "), "{}", lines[1]);
    assert!(
        lines[1].contains("1 runs, 2 turns, 0 model reviews"),
        "{}",
        lines[1]
    );
    assert!(lines[1].contains("$0.40"), "{}", lines[1]);
    assert_eq!(
        lines[2],
        "blyte $0.70 (orchestrator $0.00, implementers $0.70 in 2 runs)"
    );

    let out_json = blyte_cmd(
        dir.path(),
        &[
            "cost",
            "--by-session",
            "--repo",
            "/repo/a",
            "--since",
            "1700000100",
            "--json",
        ],
        Some(&log_path),
    );
    assert_eq!(out_json.status.code(), Some(0));
    let val: serde_json::Value = serde_json::from_slice(&out_json.stdout).unwrap();
    let sessions = val["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{val}");
    assert_eq!(sessions[0]["session"], "sess-early");
    assert_eq!(sessions[0]["first_ts"], 1_700_000_100);
    assert_eq!(sessions[0]["last_ts"], 1_700_000_300);
    assert_eq!(sessions[0]["runs"], 1);
    assert_eq!(sessions[0]["turns"], 2);
    assert_eq!(sessions[0]["model_reviews"], 1);
    assert!((sessions[0]["cost_usd"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert_eq!(sessions[1]["session"], "sess-late");
    assert_eq!(sessions[1]["first_ts"], 1_700_000_200);
    assert_eq!(sessions[1]["last_ts"], 1_700_000_200);
    assert_eq!(sessions[1]["runs"], 1);
    assert_eq!(sessions[1]["turns"], 2);
    assert_eq!(sessions[1]["model_reviews"], 0);
    assert!((sessions[1]["cost_usd"].as_f64().unwrap() - 0.40).abs() < 1e-9);
    assert_eq!(val["runs"], 2);
    assert!((val["implementers_usd"].as_f64().unwrap() - 0.70).abs() < 1e-9);
    assert!((val["orchestrator_usd"].as_f64().unwrap() - 0.0).abs() < 1e-9);
}

#[test]
fn rows_without_a_session_are_listed_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("usage.jsonl");

    let rows = vec![
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_100,
            repo: "/repo/a".into(),
            session: None,
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 3,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd: Some(0.25),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_200,
            repo: "/repo/a".into(),
            session: None,
            dispatch: Some("d1".into()),
            research: false,
            model: "m1".into(),
            outcome: "model_review".into(),
            turns: 1,
            input: 50,
            cached: 0,
            output: 20,
            thinking: 0,
            cost_usd: Some(0.05),
        },
        blyte_cli::usage_log::Entry {
            ts: 1_700_000_300,
            repo: "/repo/a".into(),
            session: Some("sess-1".into()),
            dispatch: None,
            research: false,
            model: "m1".into(),
            outcome: "verified".into(),
            turns: 1,
            input: 50,
            cached: 0,
            output: 20,
            thinking: 0,
            cost_usd: Some(0.10),
        },
    ];
    for row in &rows {
        blyte_cli::usage_log::append(&log_path, row).unwrap();
    }

    let out_text = blyte_cmd(dir.path(), &["cost", "--by-session"], Some(&log_path));
    assert_eq!(out_text.status.code(), Some(0));
    let stdout = String::from_utf8(out_text.stdout).unwrap();
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 3, "stdout:\n{stdout}");
    assert!(lines[0].starts_with("(none)  "), "{}", lines[0]);
    assert!(
        lines[0].contains("1 runs, 3 turns, 1 model reviews"),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("$0.30"), "{}", lines[0]);
    assert!(lines[1].starts_with("sess-1  "), "{}", lines[1]);
    assert!(
        lines[1].contains("0 runs, 1 turns, 0 model reviews"),
        "{}",
        lines[1]
    );
    assert!(lines[1].contains("$0.10"), "{}", lines[1]);
    assert_eq!(
        lines[2],
        "blyte $0.40 (orchestrator $0.10, implementers $0.30 in 1 runs)"
    );

    let out_json = blyte_cmd(
        dir.path(),
        &["cost", "--by-session", "--json"],
        Some(&log_path),
    );
    assert_eq!(out_json.status.code(), Some(0));
    let val: serde_json::Value = serde_json::from_slice(&out_json.stdout).unwrap();
    let sessions = val["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["session"], "(none)");
    assert_eq!(sessions[0]["runs"], 1);
    assert_eq!(sessions[0]["turns"], 3);
    assert_eq!(sessions[0]["model_reviews"], 1);
    assert!((sessions[0]["cost_usd"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert_eq!(sessions[1]["session"], "sess-1");
}
