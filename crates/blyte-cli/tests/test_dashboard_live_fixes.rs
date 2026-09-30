//! Fixes from the dashboard's first live run: reused PIDs and agy summaries (2026-09-30).

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{collect, parse_etime, Sources};

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

fn job<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["key"] == key)
        .unwrap_or_else(|| panic!("no job {key} in {v}"))
}

fn sources(dir: &Path) -> Sources {
    Sources {
        roots: vec![],
        runs_dirs: vec![dir.join("runs")],
        hours: 24,
    }
}

#[test]
fn etime_parses_every_ps_format() {
    assert_eq!(parse_etime("00:07"), Some(7));
    assert_eq!(parse_etime("  12:34\n"), Some(754));
    assert_eq!(parse_etime("01:02:03"), Some(3723));
    assert_eq!(parse_etime("2-01:02:03"), Some(2 * 86400 + 3723));
    assert_eq!(parse_etime(""), None);
    assert_eq!(parse_etime("soon"), None);
}

#[test]
fn a_live_pid_that_started_after_the_job_is_not_the_job() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs/repo-c");
    // This test process is alive, but it started long after a job begun two days ago.
    write(
        &runs.join("OLDPID.log"),
        "[Turn 2] \n● read a.rs -> 1 lines\n",
    );
    write(&runs.join("OLDPID.pid"), &std::process::id().to_string());
    let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 86400);
    std::fs::File::options()
        .write(true)
        .open(runs.join("OLDPID.pid"))
        .unwrap()
        .set_modified(two_days_ago)
        .unwrap();

    let v = collect(&sources(tmp.path()), now());
    assert_eq!(job(&v, "repo-c/OLDPID")["status"], "stopped");
    assert_eq!(v["summary"]["running"], 0);
}

#[test]
fn agy_summaries_give_finished_and_error_jobs_their_details() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs/repo-d");
    write(&runs.join("A1.log"), "agy output\n");
    write(&runs.join("A1.prompt"), "Task A1: a thing\n\nbody\n");
    write(
        &runs.join("A1.json"),
        r#"{"status":"SUCCESS","duration_seconds":553.4,"num_turns":4,"response":"Done: the thing\nmore"}"#,
    );
    write(&runs.join("A2.log"), "agy output\n");
    write(
        &runs.join("A2.json"),
        r#"{"status":"ERROR","error":"Internal error encountered.","duration_seconds":12.0,"num_turns":1,"response":""}"#,
    );

    let v = collect(&sources(tmp.path()), now());
    let a1 = job(&v, "repo-d/A1");
    assert_eq!(a1["status"], "finished");
    assert_eq!(a1["elapsed_s"], 553);
    assert_eq!(a1["turns"], 4);
    assert_eq!(a1["now"], "Done: the thing");
    assert!(a1["finished_at"].as_u64().is_some());
    let a2 = job(&v, "repo-d/A2");
    assert_eq!(a2["status"], "error");
    assert_eq!(a2["now"], "Internal error encountered.");
}
