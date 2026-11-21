use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::replay::{card_for_run, card_of, collect, group_of};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn sources_for(root: &Path) -> Sources {
    Sources {
        roots: vec![root.to_path_buf()],
        runs_dirs: vec![],
        hours: 0,
    }
}

#[test]
fn group_of_picks_most_common_group_with_tie_breaker() {
    let files: Vec<String> = ["crates/a/src/x.rs", "crates/a/src/y.rs", "docs/z.md"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(group_of(&files).as_deref(), Some("crates/a"));

    let tie: Vec<String> = ["docs/a.md", "src/b.rs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(group_of(&tie).as_deref(), Some("docs"));

    let root_file = vec!["README.md".to_string()];
    assert_eq!(group_of(&root_file).as_deref(), Some("."));

    let crates_readme = vec!["crates/README.md".to_string()];
    assert_eq!(group_of(&crates_readme).as_deref(), Some("crates"));

    assert_eq!(group_of(&[]), None);
}

#[test]
fn card_of_classifies_clean_and_exception() {
    assert_eq!(card_of("verified", true, &[]), "clean");
    assert_eq!(card_of("verified", false, &[]), "exception");
    assert_eq!(
        card_of("verified", true, &["flag one".to_string()]),
        "exception"
    );
    assert_eq!(card_of("gate_failed", true, &[]), "exception");
}

#[test]
fn verified_run_with_summary_checks_and_verdict() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let started_at = t - 1000;
    let verdict_ts = t - 400;
    let wt = d.path().join("repos/repo-a/.wt");
    let task_path = d.path().join("tasks/t1.md");

    write(&task_path, "# Build replay data\n\nDetails.\n");
    write(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":999999,"started_at":{started_at},"task_file":"{}","plan":"docs/plans/x.md","plan_task":"Task T1","research":false}}"#,
            task_path.display()
        ),
    );
    write(
        &wt.join("T1.log"),
        "[Turn 18] \n✓ Gate passed (attempt 1)\n",
    );
    write(
        &wt.join("T1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.14,"turns":18,"gate_attempts":1,"model_id":"m","gate_failures":[]}"#,
    );
    write(
        &wt.join("T1.json"),
        r#"{"outcome":"verified","seconds":420.9,"final_message":"the implementer's report","commits":[{"sha":"abc1234","subject":"feat: x"}],"changed_files":["crates/blyte-cli/src/dispatch.rs"]}"#,
    );
    write(
        &wt.join("T1.checks.json"),
        &format!(r#"{{"ts":{},"flags":[],"fallbacks":[]}}"#, t - 500),
    );
    write(
        &wt.join("T1.verdict.json"),
        &format!(r#"{{"verdict":"accepted","notes":"looks good","ts":{verdict_ts}}}"#),
    );

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert_eq!(replay.exported_at, t);
    assert_eq!(replay.window_days, 7);
    assert!(replay.skipped.is_empty());
    assert_eq!(replay.runs.len(), 1);

    let run = &replay.runs[0];
    assert_eq!(run.key, "repo-a/T1");
    assert_eq!(run.id, "T1");
    assert_eq!(run.repo, "repo-a");
    assert!(!run.research);
    assert_eq!(run.title, "Build replay data");
    assert_eq!(run.plan_file.as_deref(), Some("docs/plans/x.md"));
    assert_eq!(run.plan_task.as_deref(), Some("Task T1"));
    assert_eq!(run.started_at, started_at);
    assert_eq!(run.finished_at, started_at + 420);
    assert_eq!(run.outcome, "verified");
    assert_eq!(
        run.final_message.as_deref(),
        Some("the implementer's report")
    );
    assert_eq!(run.turns, 18);
    assert_eq!(run.cost_usd, Some(0.14));
    assert_eq!(run.commits.len(), 1);
    assert_eq!(run.group.as_deref(), Some("crates/blyte-cli"));
    assert!(run.checks_recorded);
    assert!(run.flags.is_empty());
    assert_eq!(run.card, "clean");
    assert_eq!(run.verdict.as_deref(), Some("accepted"));
    assert_eq!(run.verdict_notes.as_deref(), Some("looks good"));
    assert_eq!(run.verdict_ts, Some(verdict_ts));
}

#[test]
fn run_without_checks_file_is_exception() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let started_at = t - 500;
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("T2.start.json"),
        &format!(r#"{{"id":"T2","pid":999999,"started_at":{started_at}}}"#),
    );
    write(&wt.join("T2.log"), "[Turn 4] \n");
    write(
        &wt.join("T2.json"),
        r#"{"outcome":"verified","seconds":100,"commits":[],"changed_files":[]}"#,
    );

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.skipped.is_empty());
    assert_eq!(replay.runs.len(), 1);

    let run = &replay.runs[0];
    assert!(!run.checks_recorded);
    assert!(run.flags.is_empty());
    assert_eq!(run.card, "exception");
    assert_eq!(run.group, None);
    assert_eq!(run.final_message, None);
}

#[test]
fn run_without_summary_file_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("T3.start.json"),
        &format!(r#"{{"id":"T3","pid":999999,"started_at":{}}}"#, t - 300),
    );
    write(&wt.join("T3.log"), "[Turn 1] \n");

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.runs.is_empty());
    assert_eq!(replay.skipped.len(), 1);
    assert_eq!(replay.skipped[0].key, "repo-a/T3");
    assert_eq!(replay.skipped[0].reason, "no summary file");
}

#[test]
fn unreadable_summary_file_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("T4.start.json"),
        &format!(r#"{{"id":"T4","pid":999999,"started_at":{}}}"#, t - 300),
    );
    write(&wt.join("T4.log"), "[Turn 1] \n");
    write(&wt.join("T4.json"), "not json");

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.runs.is_empty());
    assert_eq!(replay.skipped.len(), 1);
    assert_eq!(replay.skipped[0].key, "repo-a/T4");
    assert!(
        replay.skipped[0].reason.starts_with("summary unreadable:"),
        "got: {}",
        replay.skipped[0].reason
    );
}

#[test]
fn unreadable_checks_file_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("T5.start.json"),
        &format!(r#"{{"id":"T5","pid":999999,"started_at":{}}}"#, t - 300),
    );
    write(&wt.join("T5.log"), "[Turn 1] \n");
    write(
        &wt.join("T5.json"),
        r#"{"outcome":"verified","seconds":60}"#,
    );
    write(&wt.join("T5.checks.json"), "{}");

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.runs.is_empty());
    assert_eq!(replay.skipped.len(), 1);
    assert_eq!(replay.skipped[0].key, "repo-a/T5");
    assert!(
        replay.skipped[0].reason.starts_with("checks unreadable:"),
        "got: {}",
        replay.skipped[0].reason
    );
}

#[test]
fn runs_are_sorted_by_started_at_regardless_of_id() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("A1.start.json"),
        &format!(r#"{{"id":"A1","pid":999999,"started_at":{}}}"#, t - 100),
    );
    write(&wt.join("A1.log"), "[Turn 1] \n");
    write(
        &wt.join("A1.json"),
        r#"{"outcome":"verified","seconds":10}"#,
    );

    write(
        &wt.join("Z9.start.json"),
        &format!(r#"{{"id":"Z9","pid":999999,"started_at":{}}}"#, t - 500),
    );
    write(&wt.join("Z9.log"), "[Turn 1] \n");
    write(
        &wt.join("Z9.json"),
        r#"{"outcome":"verified","seconds":20}"#,
    );

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    let ids: Vec<&str> = replay.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["Z9", "A1"]);
}

#[test]
fn run_older_than_window_is_absent_from_both_lists() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let old_ts = t - 8 * 86_400;
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("OLD.start.json"),
        &format!(r#"{{"id":"OLD","pid":999999,"started_at":{old_ts}}}"#),
    );
    write(&wt.join("OLD.log"), "[Turn 1] \n");
    write(
        &wt.join("OLD.report.json"),
        r#"{"outcome":"verified","turns":1}"#,
    );
    write(
        &wt.join("OLD.json"),
        r#"{"outcome":"verified","seconds":60}"#,
    );

    let old_time = SystemTime::now() - Duration::from_secs(8 * 86_400);
    for f in ["OLD.log", "OLD.report.json"] {
        std::fs::File::options()
            .write(true)
            .open(wt.join(f))
            .unwrap()
            .set_modified(old_time)
            .unwrap();
    }

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.runs.is_empty());
    assert!(replay.skipped.is_empty());
}

#[test]
fn serialized_run_has_exact_contract_keys() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("T1.start.json"),
        &format!(r#"{{"id":"T1","pid":999999,"started_at":{}}}"#, t - 200),
    );
    write(&wt.join("T1.log"), "[Turn 1] \n");
    write(
        &wt.join("T1.json"),
        r#"{"outcome":"verified","seconds":30}"#,
    );

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert_eq!(replay.runs.len(), 1);

    let val = serde_json::to_value(&replay.runs[0]).unwrap();
    let actual_keys: BTreeSet<&str> = val
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected_keys: BTreeSet<&str> = [
        "key",
        "id",
        "repo",
        "research",
        "title",
        "task",
        "plan_file",
        "plan_task",
        "request",
        "started_at",
        "finished_at",
        "outcome",
        "final_message",
        "turns",
        "cost_usd",
        "commits",
        "changed_files",
        "group",
        "checks_recorded",
        "flags",
        "card",
        "verdict",
        "verdict_notes",
        "verdict_ts",
        "report",
    ]
    .into_iter()
    .collect();
    assert_eq!(actual_keys, expected_keys);
}

#[test]
fn skip_reasons_order_and_sorting() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let me = std::process::id();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("S1.start.json"),
        &format!(r#"{{"id":"S1","pid":{me}}}"#),
    );
    write(&wt.join("S1.log"), "[Turn 1] \n");

    write(&wt.join("S2.start.json"), r#"{"id":"S2","pid":999999}"#);
    write(&wt.join("S2.log"), "[Turn 1] \n");

    write(
        &wt.join("S3.start.json"),
        &format!(r#"{{"id":"S3","pid":999999,"started_at":{}}}"#, t - 200),
    );
    write(&wt.join("S3.log"), "[Turn 1] \n");
    write(&wt.join("S3.json"), r#"{"outcome":"verified"}"#);

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.runs.is_empty());
    assert_eq!(replay.skipped.len(), 3);
    assert_eq!(replay.skipped[0].key, "repo-a/S1");
    assert_eq!(replay.skipped[0].reason, "still running");
    assert_eq!(replay.skipped[1].key, "repo-a/S2");
    assert_eq!(replay.skipped[1].reason, "no start time");
    assert_eq!(replay.skipped[2].key, "repo-a/S3");
    assert!(replay.skipped[2].reason.starts_with("summary unreadable:"));
}

#[test]
fn flagged_checks_produce_exception_card_and_no_root_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("F1.start.json"),
        &format!(r#"{{"id":"F1","pid":999999,"started_at":{}}}"#, t - 200),
    );
    write(&wt.join("F1.log"), "[Turn 1] \n");
    write(
        &wt.join("F1.json"),
        r#"{"outcome":"verified","seconds":30}"#,
    );
    write(&wt.join("F1.checks.json"), r#"{"flags":["dirty tree"]}"#);

    write(&d.path().join("runs/repo-b/R0.log"), "[Turn 1] \n");

    let src = Sources {
        roots: vec![d.path().join("repos")],
        runs_dirs: vec![d.path().join("runs")],
        hours: 0,
    };
    let replay = collect(&src, t, 7);
    assert_eq!(replay.runs.len(), 1);
    assert!(replay.runs[0].checks_recorded);
    assert_eq!(replay.runs[0].flags, vec!["dirty tree".to_string()]);
    assert_eq!(replay.runs[0].card, "exception");

    assert_eq!(replay.skipped.len(), 1);
    assert_eq!(replay.skipped[0].key, "repo-b/R0");
    assert_eq!(replay.skipped[0].reason, "no root");
}

#[test]
fn card_for_run_classifies_research_independently_of_outcome_and_checks() {
    assert_eq!(card_for_run(true, "verified", true, &[]), "research");
    assert_eq!(card_for_run(true, "verified", false, &[]), "research");
    assert_eq!(
        card_for_run(
            true,
            "budget_exhausted",
            false,
            &["broad research run".to_string()],
        ),
        "research"
    );

    let flags = vec!["flag one".to_string()];
    assert_eq!(
        card_for_run(false, "verified", true, &[]),
        card_of("verified", true, &[])
    );
    assert_eq!(
        card_for_run(false, "verified", false, &[]),
        card_of("verified", false, &[])
    );
    assert_eq!(
        card_for_run(false, "verified", true, &flags),
        card_of("verified", true, &flags)
    );
    assert_eq!(
        card_for_run(false, "gate_failed", true, &[]),
        card_of("gate_failed", true, &[])
    );
}

#[test]
fn research_run_gets_research_card_and_report_markdown() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");
    let markdown = "# Findings\n\n- Key insight one\n- Key insight two\n";

    write(
        &wt.join("R1.start.json"),
        &format!(
            r#"{{"id":"R1","pid":999999,"started_at":{},"research":true}}"#,
            t - 300
        ),
    );
    write(&wt.join("R1.log"), "[Turn 5] \n");
    write(
        &wt.join("R1.json"),
        r#"{"outcome":"verified","seconds":90,"commits":[],"changed_files":[]}"#,
    );
    write(&wt.join("R1.report.md"), markdown);

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.skipped.is_empty());
    assert_eq!(replay.runs.len(), 1);

    let run = &replay.runs[0];
    assert!(run.research);
    assert_eq!(run.card, "research");
    assert_eq!(run.report.as_deref(), Some(markdown));
}

#[test]
fn research_run_without_report_file_has_none_report_and_research_card() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("R2.start.json"),
        &format!(
            r#"{{"id":"R2","pid":999999,"started_at":{},"research":true}}"#,
            t - 300
        ),
    );
    write(&wt.join("R2.log"), "[Turn 3] \n");
    write(
        &wt.join("R2.json"),
        r#"{"outcome":"verified","seconds":45,"commits":[],"changed_files":[]}"#,
    );

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.skipped.is_empty());
    assert_eq!(replay.runs.len(), 1);

    let run = &replay.runs[0];
    assert_eq!(run.card, "research");
    assert!(run.report.is_none());
}

#[test]
fn research_run_with_unreadable_report_file_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("RBAD.start.json"),
        &format!(
            r#"{{"id":"RBAD","pid":999999,"started_at":{},"research":true}}"#,
            t - 300
        ),
    );
    write(&wt.join("RBAD.log"), "[Turn 2] \n");
    write(
        &wt.join("RBAD.json"),
        r#"{"outcome":"verified","seconds":30,"commits":[],"changed_files":[]}"#,
    );
    std::fs::write(wt.join("RBAD.report.md"), [0xff_u8, 0xfe]).unwrap();

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.runs.is_empty());
    assert_eq!(replay.skipped.len(), 1);
    assert_eq!(replay.skipped[0].key, "repo-a/RBAD");
    assert!(
        replay.skipped[0].reason.starts_with("report unreadable: "),
        "got: {}",
        replay.skipped[0].reason
    );
}

#[test]
fn non_research_run_ignores_report_file() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");

    write(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":999999,"started_at":{},"research":false}}"#,
            t - 300
        ),
    );
    write(&wt.join("T1.log"), "[Turn 4] \n");
    write(
        &wt.join("T1.json"),
        r#"{"outcome":"verified","seconds":60,"commits":[],"changed_files":[]}"#,
    );
    write(&wt.join("T1.report.md"), "# Should be ignored\n");

    let replay = collect(&sources_for(&d.path().join("repos")), t, 7);
    assert!(replay.skipped.is_empty());
    assert_eq!(replay.runs.len(), 1);

    let run = &replay.runs[0];
    assert!(run.report.is_none());
    assert_ne!(run.card, "research");
}
