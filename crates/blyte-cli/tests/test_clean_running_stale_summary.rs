use std::path::Path;
use std::process::{Child, Command};
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

struct LiveChild(Child);

impl LiveChild {
    fn spawn() -> Self {
        Self(Command::new("sleep").arg("60").spawn().unwrap())
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for LiveChild {
    fn drop(&mut self) {
        self.stop();
    }
}

// 1. Live PID, start file started_at = now, summary .wt/<ID>.json with mtime one hour before:
//    blyte clean (and clean_quiet) keeps .wt/<ID>.
#[test]
fn live_pid_with_stale_summary_is_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", wt_path.to_str().unwrap()],
    );

    let mut child = LiveChild::spawn();
    let now = now_secs();
    let wt_dir = root.join(".wt");

    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({
            "id": "T1",
            "pid": child.pid(),
            "started_at": now,
        })
        .to_string(),
    );
    let summary_path = wt_dir.join("T1.json");
    write_file(
        &summary_path,
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    set_mtime_secs(&summary_path, now - 3600);

    blyte_cli::clean::clean_quiet(root, None);
    assert!(wt_path.exists(), "clean_quiet must keep .wt/T1");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T1 (running)"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "blyte clean must keep .wt/T1");

    child.stop();
}

// 2. Same, plus .wt/<ID>.verdict.json with verdict redone: still kept.
#[test]
fn live_pid_with_stale_summary_and_redone_verdict_is_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T2");
    git(
        root,
        &["worktree", "add", "-b", "wip/T2", wt_path.to_str().unwrap()],
    );

    let mut child = LiveChild::spawn();
    let now = now_secs();
    let wt_dir = root.join(".wt");

    write_file(
        &wt_dir.join("T2.start.json"),
        &serde_json::json!({
            "id": "T2",
            "pid": child.pid(),
            "started_at": now,
        })
        .to_string(),
    );
    let summary_path = wt_dir.join("T2.json");
    write_file(
        &summary_path,
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    set_mtime_secs(&summary_path, now - 3600);
    write_file(
        &wt_dir.join("T2.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );

    blyte_cli::clean::clean_quiet(root, None);
    assert!(wt_path.exists(), "clean_quiet must keep .wt/T2");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T2 (running)"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "blyte clean must keep .wt/T2");

    child.stop();
}

// 3. Live PID, summary mtime after started_at: removed, as before (a finished run whose PID was reused).
#[test]
fn live_pid_with_summary_mtime_after_started_at_is_removed() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T3");
    git(
        root,
        &["worktree", "add", "-b", "wip/T3", wt_path.to_str().unwrap()],
    );

    let mut child = LiveChild::spawn();
    let now = now_secs();
    let wt_dir = root.join(".wt");

    write_file(
        &wt_dir.join("T3.start.json"),
        &serde_json::json!({
            "id": "T3",
            "pid": child.pid(),
            "started_at": now - 3600,
        })
        .to_string(),
    );
    let summary_path = wt_dir.join("T3.json");
    write_file(
        &summary_path,
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    set_mtime_secs(&summary_path, now);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(stdout.contains("removed .wt/T3"), "stdout was: {stdout}");
    assert!(!wt_path.exists(), ".wt/T3 should be removed");

    child.stop();
}

// 4. Dead PID (999999), stale summary: removed, as before.
#[test]
fn dead_pid_with_stale_summary_is_removed() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T4");
    git(
        root,
        &["worktree", "add", "-b", "wip/T4", wt_path.to_str().unwrap()],
    );

    let now = now_secs();
    let wt_dir = root.join(".wt");

    write_file(
        &wt_dir.join("T4.start.json"),
        &serde_json::json!({
            "id": "T4",
            "pid": 999999,
            "started_at": now,
        })
        .to_string(),
    );
    let summary_path = wt_dir.join("T4.json");
    write_file(
        &summary_path,
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    set_mtime_secs(&summary_path, now - 3600);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(stdout.contains("removed .wt/T4"), "stdout was: {stdout}");
    assert!(!wt_path.exists(), ".wt/T4 should be removed");
}

// 5. Live PID, summary exists, start file has no started_at: kept.
#[test]
fn live_pid_with_summary_and_no_started_at_is_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T5");
    git(
        root,
        &["worktree", "add", "-b", "wip/T5", wt_path.to_str().unwrap()],
    );

    let mut child = LiveChild::spawn();
    let wt_dir = root.join(".wt");

    write_file(
        &wt_dir.join("T5.start.json"),
        &serde_json::json!({
            "id": "T5",
            "pid": child.pid(),
        })
        .to_string(),
    );
    write_file(
        &wt_dir.join("T5.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    blyte_cli::clean::clean_quiet(root, None);
    assert!(wt_path.exists(), "clean_quiet must keep .wt/T5");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T5 (running)"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "blyte clean must keep .wt/T5");

    child.stop();
}
