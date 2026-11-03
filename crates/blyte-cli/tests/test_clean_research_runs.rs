use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@test.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn init_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q", "-b", "main"]);
    write_file(&t.path().join("README.md"), "initial\n");
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "initial", "--no-gpg-sign"]);
    t
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn set_mtime_secs(path: &Path, secs: u64) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let stamp = format!("{y:04}{m:02}{d:02}{hour:02}{min:02}.{sec:02}");
    let out = Command::new("touch")
        .env("TZ", "UTC0")
        .args(["-t", &stamp, path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "touch -t {stamp} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn research_run_with_report_in_worktree_copies_report_and_removes_worktree() {
    let repo = init_repo();
    let root = repo.path();

    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join("R1");
    std::fs::create_dir_all(&wt_path).unwrap();
    let report_contents = "# Research Findings\n\nDetailed report here.\n";
    write_file(&wt_path.join("report.md"), report_contents);

    write_file(
        &wt_dir.join("R1.start.json"),
        &serde_json::json!({ "id": "R1", "pid": 999999, "research": true }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/R1 (research report (copied out on removal),"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree should be removed");
    assert_eq!(
        std::fs::read_to_string(wt_dir.join("R1.report.md")).unwrap(),
        report_contents
    );
}

#[test]
fn research_run_with_report_in_worktree_dry_run_copies_and_removes_nothing() {
    let repo = init_repo();
    let root = repo.path();

    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join("R2");
    std::fs::create_dir_all(&wt_path).unwrap();
    let report_contents = "# Dry Run Report\n";
    write_file(&wt_path.join("report.md"), report_contents);

    write_file(
        &wt_dir.join("R2.start.json"),
        &serde_json::json!({ "id": "R2", "pid": 999999, "research": true }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--dry-run"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean --dry-run failed: {stdout}");
    assert!(
        stdout.contains("would remove .wt/R2 (research report (copied out on removal),"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "worktree must remain after dry-run");
    assert!(
        !wt_dir.join("R2.report.md").exists(),
        "report must not be copied out in dry-run"
    );
}

#[test]
fn research_run_with_summary_error_and_no_report_is_removed() {
    let repo = init_repo();
    let root = repo.path();

    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join("R3");
    std::fs::create_dir_all(&wt_path).unwrap();
    write_file(&wt_path.join("scratch.txt"), "notes\n");

    write_file(
        &wt_dir.join("R3.start.json"),
        &serde_json::json!({ "id": "R3", "pid": 999999, "research": true }).to_string(),
    );
    write_file(
        &wt_dir.join("R3.json"),
        &serde_json::json!({ "outcome": "error" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("ended without a report (error)"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree should be removed");
}

#[test]
fn research_run_with_no_summary_removed_when_old_kept_when_fresh() {
    let repo = init_repo();
    let root = repo.path();

    let wt_dir = root.join(".wt");
    let now = now_secs();

    // Old start file (20 days back): removed
    let wt_old = wt_dir.join("R_OLD");
    std::fs::create_dir_all(&wt_old).unwrap();
    write_file(&wt_old.join("scratch.txt"), "old scratch\n");
    let old_start = wt_dir.join("R_OLD.start.json");
    write_file(
        &old_start,
        &serde_json::json!({ "id": "R_OLD", "pid": 999999, "research": true }).to_string(),
    );
    set_mtime_secs(&old_start, now - 20 * 86400);

    // Fresh start file: kept as "no report"
    let wt_fresh = wt_dir.join("R_FRESH");
    std::fs::create_dir_all(&wt_fresh).unwrap();
    write_file(&wt_fresh.join("scratch.txt"), "fresh scratch\n");
    let fresh_start = wt_dir.join("R_FRESH.start.json");
    write_file(
        &fresh_start,
        &serde_json::json!({ "id": "R_FRESH", "pid": 999999, "research": true }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains(
            "removed .wt/R_OLD (research run stopped without a summary, started more than 14 days ago,"
        ),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/R_FRESH (no report)"),
        "stdout was: {stdout}"
    );
    assert!(!wt_old.exists(), "old research run should be removed");
    assert!(wt_fresh.exists(), "fresh research run should be kept");
}

#[test]
fn research_run_with_summary_missing_outcome_is_kept_with_unreadable_summary() {
    let repo = init_repo();
    let root = repo.path();

    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join("R5");
    std::fs::create_dir_all(&wt_path).unwrap();
    write_file(&wt_path.join("scratch.txt"), "notes\n");

    write_file(
        &wt_dir.join("R5.start.json"),
        &serde_json::json!({ "id": "R5", "pid": 999999, "research": true }).to_string(),
    );
    write_file(
        &wt_dir.join("R5.json"),
        &serde_json::json!({ "id": "R5" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/R5 (unreadable summary)"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "worktree should be kept");
}

#[test]
fn non_research_run_with_no_summary_and_old_start_file_is_kept_as_before() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", wt_path.to_str().unwrap()],
    );
    write_file(&wt_path.join("work.txt"), "work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    let start_path = wt_dir.join("T1.start.json");
    write_file(
        &start_path,
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    let now = now_secs();
    set_mtime_secs(&start_path, now - 20 * 86400);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T1 (no verdict)"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "non-research worktree must be kept");
}

#[test]
fn research_run_copy_failure_prints_error_and_keeps_worktree() {
    let repo = init_repo();
    let root = repo.path();

    let wt_dir = root.join(".wt");
    let wt_path = wt_dir.join("R_ERR");
    std::fs::create_dir_all(&wt_path).unwrap();
    write_file(&wt_path.join("report.md"), "# Report\n");
    write_file(
        &wt_dir.join("R_ERR.start.json"),
        &serde_json::json!({ "id": "R_ERR", "pid": 999999, "research": true }).to_string(),
    );

    // Make the destination a directory so std::fs::copy fails
    std::fs::create_dir_all(wt_dir.join("R_ERR.report.md")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "clean failed: {stdout} / {stderr}");
    assert!(
        stderr.contains("error: could not copy "),
        "stderr was: {stderr}"
    );
    assert!(stdout.contains("kept .wt/R_ERR"), "stdout was: {stdout}");
    assert!(stdout.contains("freed 0 B"), "stdout was: {stdout}");
    assert!(wt_path.exists(), "worktree must be kept when copy fails");
}
