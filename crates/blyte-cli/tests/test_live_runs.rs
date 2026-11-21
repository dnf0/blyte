use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::live_runs;
use blyte_cli::replay;

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

fn init_git_repo(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
}

fn sources_for(root: &Path) -> Sources {
    Sources {
        roots: vec![root.to_path_buf()],
        runs_dirs: vec![],
        hours: 0,
    }
}

#[test]
fn finished_runs_match_replay() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let repo = d.path().join("repo-a");
    init_git_repo(&repo);
    let wt = repo.join(".wt");

    let code_task = d.path().join("tasks/t1.md");
    write(&code_task, "# Build live runs\n\nDetails.\n");
    write(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":999999,"started_at":{},"task_file":"{}","plan":"docs/plans/x.md","plan_task":"Task T1","research":false}}"#,
            t - 1000,
            code_task.display()
        ),
    );
    write(
        &wt.join("T1.log"),
        "[Turn 12] \n✓ Gate passed (attempt 1)\n",
    );
    write(
        &wt.join("T1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.22,"turns":12,"gate_attempts":1,"model_id":"m","gate_failures":[]}"#,
    );
    write(
        &wt.join("T1.json"),
        r#"{"outcome":"verified","seconds":300,"final_message":"done","commits":[{"sha":"abc1234","subject":"feat: x"}],"changed_files":["crates/blyte-cli/src/live_runs.rs"]}"#,
    );
    write(
        &wt.join("T1.checks.json"),
        &format!(r#"{{"ts":{},"flags":[],"fallbacks":[]}}"#, t - 650),
    );
    write(
        &wt.join("T1.verdict.json"),
        &format!(
            r#"{{"verdict":"accepted","notes":"good","ts":{}}}"#,
            t - 600
        ),
    );

    let research_task = d.path().join("tasks/r1.md");
    write(&research_task, "# Explore queue shape\n\nQuestions.\n");
    write(
        &wt.join("R1.start.json"),
        &format!(
            r#"{{"id":"R1","pid":999999,"started_at":{},"task_file":"{}","research":true}}"#,
            t - 500,
            research_task.display()
        ),
    );
    write(&wt.join("R1.log"), "[Turn 5] \n");
    write(
        &wt.join("R1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.08,"turns":5,"gate_attempts":0,"model_id":"m","gate_failures":[]}"#,
    );
    write(
        &wt.join("R1.json"),
        r#"{"outcome":"verified","seconds":90,"commits":[],"changed_files":[]}"#,
    );
    write(&wt.join("R1.report.md"), "# Findings\n\n- Insight one\n");

    let live = live_runs::collect(&repo, t, 7).unwrap();
    let mut rep = replay::collect(&sources_for(&repo), t, 7);
    rep.runs.sort_by(|a, b| {
        b.finished_at
            .cmp(&a.finished_at)
            .then_with(|| a.key.cmp(&b.key))
    });

    assert_eq!(live.generated_at, t);
    assert_eq!(live.repo, "repo-a");
    assert!(live.running.is_empty());
    assert!(live.skipped.is_empty());
    assert_eq!(live.runs.len(), 2);
    assert_eq!(
        serde_json::to_value(&live.runs).unwrap(),
        serde_json::to_value(&rep.runs).unwrap()
    );
}

#[test]
fn running_run_is_listed_not_skipped() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let me = std::process::id();
    let started_at = t - 120;
    let repo = d.path().join("repo-a");
    init_git_repo(&repo);
    let wt = repo.join(".wt");

    let task_path = d.path().join("tasks/s1.md");
    write(&task_path, "# Stream live status\n\nWork in progress.\n");
    write(
        &wt.join("S1.start.json"),
        &format!(
            r#"{{"id":"S1","pid":{me},"started_at":{started_at},"task_file":"{}","plan":"docs/plans/p.md","plan_task":"Task S1","research":false}}"#,
            task_path.display()
        ),
    );
    write(
        &wt.join("S1.log"),
        "[Turn 7] \n● bash: cargo test\n⏳ Gate running\n",
    );

    let live = live_runs::collect(&repo, t, 7).unwrap();
    assert!(live.runs.is_empty());
    assert!(live.skipped.is_empty());
    assert_eq!(live.running.len(), 1);

    let run = &live.running[0];
    assert_eq!(run.key, "repo-a/S1");
    assert_eq!(run.id, "S1");
    assert!(!run.research);
    assert_eq!(run.title, "Stream live status");
    assert!(run.task.contains("Work in progress."));
    assert_eq!(run.plan_file.as_deref(), Some("docs/plans/p.md"));
    assert_eq!(run.plan_task.as_deref(), Some("Task S1"));
    assert_eq!(run.started_at, Some(started_at));
    assert_eq!(run.turns, 7);
    assert!(run.gate_running);
    assert_eq!(run.activity, "bash: cargo test");
}

#[test]
fn other_repositories_are_left_out() {
    let d = tempfile::tempdir().unwrap();
    let t = now();

    let repo_a = d.path().join("repo-a");
    init_git_repo(&repo_a);
    let wt_a = repo_a.join(".wt");
    write(
        &wt_a.join("A1.start.json"),
        &format!(r#"{{"id":"A1","pid":999999,"started_at":{}}}"#, t - 300),
    );
    write(&wt_a.join("A1.log"), "[Turn 2] \n");
    write(
        &wt_a.join("A1.json"),
        r#"{"outcome":"verified","seconds":30}"#,
    );

    let repo_b = d.path().join("repo-b");
    init_git_repo(&repo_b);
    let wt_b = repo_b.join(".wt");
    write(
        &wt_b.join("B1.start.json"),
        &format!(r#"{{"id":"B1","pid":999999,"started_at":{}}}"#, t - 200),
    );
    write(&wt_b.join("B1.log"), "[Turn 2] \n");
    write(
        &wt_b.join("B1.json"),
        r#"{"outcome":"verified","seconds":20}"#,
    );

    let nested = repo_a.join("nested-repo");
    init_git_repo(&nested);
    let wt_nested = nested.join(".wt");
    write(
        &wt_nested.join("N1.start.json"),
        &format!(r#"{{"id":"N1","pid":999999,"started_at":{}}}"#, t - 100),
    );
    write(&wt_nested.join("N1.log"), "[Turn 2] \n");
    write(
        &wt_nested.join("N1.json"),
        r#"{"outcome":"verified","seconds":15}"#,
    );

    let live = live_runs::collect(&repo_a, t, 7).unwrap();
    assert_eq!(live.repo, "repo-a");
    assert!(live.running.is_empty());
    assert!(live.skipped.is_empty());
    assert_eq!(live.runs.len(), 1);
    assert_eq!(live.runs[0].key, "repo-a/A1");
}

#[test]
fn unreadable_run_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let repo = d.path().join("repo-a");
    init_git_repo(&repo);
    let wt = repo.join(".wt");

    write(
        &wt.join("OK1.start.json"),
        &format!(r#"{{"id":"OK1","pid":999999,"started_at":{}}}"#, t - 400),
    );
    write(&wt.join("OK1.log"), "[Turn 3] \n");
    write(
        &wt.join("OK1.json"),
        r#"{"outcome":"verified","seconds":60}"#,
    );

    write(
        &wt.join("BAD1.start.json"),
        &format!(r#"{{"id":"BAD1","pid":999999,"started_at":{}}}"#, t - 300),
    );
    write(&wt.join("BAD1.log"), "[Turn 1] \n");
    write(&wt.join("BAD1.json"), "not valid json");

    let live = live_runs::collect(&repo, t, 7).unwrap();
    assert!(live.running.is_empty());
    assert_eq!(live.runs.len(), 1);
    assert_eq!(live.runs[0].id, "OK1");
    assert_eq!(live.skipped.len(), 1);
    assert_eq!(live.skipped[0].key, "repo-a/BAD1");
    assert!(
        live.skipped[0].reason.starts_with("summary unreadable:"),
        "got: {}",
        live.skipped[0].reason
    );
}

#[test]
fn no_wt_directory_is_empty() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let repo = d.path().join("repo-empty");
    init_git_repo(&repo);

    let child = repo.join("sub");
    init_git_repo(&child);
    write(
        &child.join(".wt/C1.start.json"),
        &format!(r#"{{"id":"C1","pid":999999,"started_at":{}}}"#, t - 100),
    );
    write(&child.join(".wt/C1.log"), "[Turn 1] \n");
    write(
        &child.join(".wt/C1.json"),
        r#"{"outcome":"verified","seconds":10}"#,
    );

    let live = live_runs::collect(&repo, t, 7).unwrap();
    assert_eq!(live.generated_at, t);
    assert_eq!(live.repo, "repo-empty");
    assert!(live.running.is_empty());
    assert!(live.runs.is_empty());
    assert!(live.skipped.is_empty());
}

#[test]
fn missing_root_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let t = now();

    let missing = d.path().join("does-not-exist");
    let err = live_runs::collect(&missing, t, 7).unwrap_err();
    assert!(
        err.contains(&missing.display().to_string()),
        "error should name missing root, got: {err}"
    );

    let file = d.path().join("a-file");
    write(&file, "content");
    let err_file = live_runs::collect(&file, t, 7).unwrap_err();
    assert!(
        err_file.contains(&file.display().to_string()),
        "error should name non-dir root, got: {err_file}"
    );
}

#[test]
fn order() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let me = std::process::id();
    let repo = d.path().join("repo-a");
    init_git_repo(&repo);
    let wt = repo.join(".wt");

    write(
        &wt.join("F1.start.json"),
        &format!(r#"{{"id":"F1","pid":999999,"started_at":{}}}"#, t - 1000),
    );
    write(&wt.join("F1.log"), "[Turn 1] \n");
    write(
        &wt.join("F1.json"),
        r#"{"outcome":"verified","seconds":900}"#,
    );

    write(
        &wt.join("F2.start.json"),
        &format!(r#"{{"id":"F2","pid":999999,"started_at":{}}}"#, t - 500),
    );
    write(&wt.join("F2.log"), "[Turn 1] \n");
    write(
        &wt.join("F2.json"),
        r#"{"outcome":"verified","seconds":200}"#,
    );

    write(
        &wt.join("F3.start.json"),
        &format!(r#"{{"id":"F3","pid":999999,"started_at":{}}}"#, t - 450),
    );
    write(&wt.join("F3.log"), "[Turn 1] \n");
    write(
        &wt.join("F3.json"),
        r#"{"outcome":"verified","seconds":10}"#,
    );

    write(
        &wt.join("R_NEW.start.json"),
        &format!(r#"{{"id":"R_NEW","pid":{me},"started_at":{}}}"#, t - 50),
    );
    write(&wt.join("R_NEW.log"), "[Turn 1] \n");

    write(
        &wt.join("R_OLD.start.json"),
        &format!(r#"{{"id":"R_OLD","pid":{me},"started_at":{}}}"#, t - 400),
    );
    write(&wt.join("R_OLD.log"), "[Turn 2] \n");

    write(
        &wt.join("R_MID.start.json"),
        &format!(r#"{{"id":"R_MID","pid":{me},"started_at":{}}}"#, t - 200),
    );
    write(&wt.join("R_MID.log"), "[Turn 3] \n");

    let live = live_runs::collect(&repo, t, 7).unwrap();
    let run_ids: Vec<&str> = live.runs.iter().map(|r| r.id.as_str()).collect();
    let running_ids: Vec<&str> = live.running.iter().map(|r| r.id.as_str()).collect();

    assert_eq!(run_ids, vec!["F1", "F2", "F3"]);
    assert_eq!(running_ids, vec!["R_OLD", "R_MID", "R_NEW"]);
}
