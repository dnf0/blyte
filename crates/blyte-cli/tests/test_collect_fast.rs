use std::fs::{File, FileTimes};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{collect_jobs, repo_name, Sources};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn set_mtime(path: &Path, time: SystemTime) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_times(FileTimes::new().set_modified(time)).unwrap();
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn repo_name_resolves_worktree_nested_and_plain_without_git_subprocess() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = tmp.path().join("alpha");
    std::fs::create_dir(&alpha).unwrap();
    run_git(&alpha, &["init", "-q"]);
    write_file(&alpha.join("f.txt"), "hello\n");
    run_git(&alpha, &["add", "."]);
    run_git(&alpha, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let alpha_wt = tmp.path().join("alpha-wt");
    run_git(
        &alpha,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/wt",
            alpha_wt.to_str().unwrap(),
        ],
    );

    let nested = alpha_wt.join("src").join("deep");
    std::fs::create_dir_all(&nested).unwrap();

    let beta = tmp.path().join("beta");
    std::fs::create_dir(&beta).unwrap();

    let gamma = tmp.path().join("gamma--feature");
    std::fs::create_dir(&gamma).unwrap();

    assert_eq!(repo_name(&alpha), "alpha");
    assert_eq!(repo_name(&alpha_wt), "alpha");
    assert_eq!(repo_name(&nested), "alpha");
    assert_eq!(repo_name(&beta), "beta");
    assert_eq!(repo_name(&gamma), "gamma");
}

#[test]
fn old_finished_runs_are_skipped_without_reading_logs_or_start_json() {
    let tmp = tempfile::tempdir().unwrap();
    let now = now_secs();
    let wt = tmp.path().join("repos/repo-fast/.wt");
    std::fs::create_dir_all(&wt).unwrap();

    let old_time = SystemTime::now() - Duration::from_secs(10 * 86_400);
    let huge_log = "[Turn 1]\n".repeat(40_000);
    let huge_invalid_start = "{not valid json ".repeat(4_000);

    for i in 0..200 {
        let id = format!("OLD{i:03}");
        let start_path = wt.join(format!("{id}.start.json"));
        let log_path = wt.join(format!("{id}.log"));
        let report_path = wt.join(format!("{id}.report.json"));

        if i % 2 == 0 {
            write_file(&start_path, &huge_invalid_start);
        } else {
            write_file(
                &start_path,
                &format!(
                    r#"{{"id":"{id}","pid":999999,"started_at":{}}}"#,
                    now.saturating_sub(10 * 86_400)
                ),
            );
        }
        write_file(&log_path, &huge_log);
        write_file(
            &report_path,
            r#"{"outcome":"verified","turns":1,"cost_usd":0.01}"#,
        );

        set_mtime(&log_path, old_time);
        set_mtime(&report_path, old_time);
    }

    for i in 0..5 {
        let id = format!("NEW{i}");
        write_file(
            &wt.join(format!("{id}.start.json")),
            &format!(
                r#"{{"id":"{id}","pid":999999,"started_at":{}}}"#,
                now.saturating_sub(600)
            ),
        );
        write_file(
            &wt.join(format!("{id}.log")),
            "[Turn 2]\n● bash cargo test\n✓ Gate passed\n",
        );
        write_file(
            &wt.join(format!("{id}.report.json")),
            r#"{"outcome":"verified","turns":2,"cost_usd":0.05}"#,
        );
    }

    let src = Sources {
        roots: vec![tmp.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let t0 = Instant::now();
    let jobs = collect_jobs(&src, now);
    let elapsed = t0.elapsed();

    assert_eq!(jobs.len(), 5);
    for job in &jobs {
        assert!(job.id.starts_with("NEW"), "unexpected job {}", job.id);
    }
    assert!(
        elapsed < Duration::from_millis(1000),
        "collect_jobs took {:?}, expected < 1000ms",
        elapsed
    );
}

#[test]
fn old_runs_dir_jobs_are_skipped_by_mtime_while_live_pid_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let now = now_secs();
    let old_time = SystemTime::now() - Duration::from_secs(10 * 86_400);
    let repo_runs = tmp.path().join("runs/repo-r");
    std::fs::create_dir_all(&repo_runs).unwrap();

    let agy_log = repo_runs.join("OLD_AGY.log");
    let agy_json = repo_runs.join("OLD_AGY.json");
    write_file(&agy_log, "old agy log\n");
    write_file(
        &agy_json,
        r#"{"status":"SUCCESS","duration_seconds":10.0,"num_turns":2,"response":"done"}"#,
    );
    set_mtime(&agy_log, old_time);
    set_mtime(&agy_json, old_time);

    let dead_log = repo_runs.join("OLD_DEAD.log");
    let dead_pid = repo_runs.join("OLD_DEAD.pid");
    write_file(&dead_log, "old dead log\n");
    write_file(&dead_pid, "999999");
    set_mtime(&dead_log, old_time);
    set_mtime(&dead_pid, old_time);

    let live_log = repo_runs.join("LIVE.log");
    let live_pid = repo_runs.join("LIVE.pid");
    write_file(&live_log, "[Turn 3]\n● read src/main.rs\n");
    write_file(&live_pid, &std::process::id().to_string());
    set_mtime(&live_log, old_time);

    let src = Sources {
        roots: vec![],
        runs_dirs: vec![tmp.path().join("runs")],
        hours: 24,
    };

    let jobs = collect_jobs(&src, now);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "LIVE");
    assert_eq!(jobs[0].status, "running");
}

#[test]
fn dispatch_live_pid_with_old_log_mtime_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let now = now_secs();
    let old_time = SystemTime::now() - Duration::from_secs(10 * 86_400);
    let wt = tmp.path().join("repos/repo-live/.wt");
    std::fs::create_dir_all(&wt).unwrap();

    let me = std::process::id();
    let log_path = wt.join("L1.log");
    write_file(
        &wt.join("L1.start.json"),
        &format!(
            r#"{{"id":"L1","pid":{me},"started_at":{}}}"#,
            now.saturating_sub(300)
        ),
    );
    write_file(&log_path, "[Turn 2]\n● edit src/lib.rs\n");
    set_mtime(&log_path, old_time);

    let src = Sources {
        roots: vec![tmp.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let jobs = collect_jobs(&src, now);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "L1");
    assert_eq!(jobs[0].status, "running");
}

#[test]
fn strip_ansi_and_parse_log_handle_plain_and_colored_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let now = now_secs();
    let wt = tmp.path().join("repos/repo-ansi/.wt");
    std::fs::create_dir_all(&wt).unwrap();

    write_file(
        &wt.join("PLAIN.start.json"),
        &format!(
            r#"{{"id":"PLAIN","pid":999999,"started_at":{}}}"#,
            now.saturating_sub(120)
        ),
    );
    write_file(&wt.join("PLAIN.log"), "[Turn 4]\n● bash\n✓ Gate passed\n");

    write_file(
        &wt.join("COLOR.start.json"),
        &format!(
            r#"{{"id":"COLOR","pid":999999,"started_at":{}}}"#,
            now.saturating_sub(120)
        ),
    );
    write_file(
        &wt.join("COLOR.log"),
        "\x1b[32m[Turn 4]\x1b[0m\n\x1b[1m● bash\x1b[0m\n\x1b[32m✓ Gate passed\x1b[0m\n",
    );

    let src = Sources {
        roots: vec![tmp.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let jobs = collect_jobs(&src, now);
    assert_eq!(jobs.len(), 2);

    let plain = jobs.iter().find(|j| j.id == "PLAIN").unwrap();
    let color = jobs.iter().find(|j| j.id == "COLOR").unwrap();

    assert_eq!(plain.turns, 4);
    assert_eq!(plain.last_tool.as_deref(), Some("bash"));
    assert_eq!(plain.gate_attempts, 1);

    assert_eq!(color.turns, plain.turns);
    assert_eq!(color.last_tool, plain.last_tool);
    assert_eq!(color.gate_attempts, plain.gate_attempts);
}
