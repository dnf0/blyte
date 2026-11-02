use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();

    let status = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(p)
        .status()
        .unwrap();
    assert!(status.success());

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(p)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(p)
        .status()
        .unwrap();

    fs::write(p.join("README.md"), "hello\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(p)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "init", "--no-gpg-sign"])
        .current_dir(p)
        .status()
        .unwrap();

    dir
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_file(path: &Path, content: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn create_merged_run(root: &Path, id: &str) {
    let wt_path = root.join(".wt").join(id);
    let branch = format!("wip/{id}");
    git(
        root,
        &["worktree", "add", "-b", &branch, wt_path.to_str().unwrap()],
    );
    write_file(&wt_path.join(format!("{id}.txt")), &format!("work {id}\n"));
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", &format!("feat: {id}"), "--no-gpg-sign"],
    );
    git(
        root,
        &["merge", "--no-ff", &branch, "-m", "merge", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join(format!("{id}.start.json")),
        &serde_json::json!({ "id": id, "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join(format!("{id}.json")),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join(format!("{id}.verdict.json")),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );
}

#[test]
fn test_clean_quiet_skip_keeps_skipped_and_prints_nothing_to_stdout() {
    let repo = init_repo();
    let root = repo.path();

    create_merged_run(root, "A");
    create_merged_run(root, "B");
    create_merged_run(root, "C");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--quiet", "--skip", "B"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "expected empty stdout, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!root.join(".wt/A").exists(), "A should be removed");
    assert!(root.join(".wt/B").exists(), "B should be kept");
    assert!(!root.join(".wt/C").exists(), "C should be removed");
}

#[test]
fn test_clean_quiet_live_lock_exits_zero_and_removes_nothing() {
    let repo = init_repo();
    let root = repo.path();

    create_merged_run(root, "A");

    let lock_path = root.join(".wt/clean.lock");
    write_file(&lock_path, &format!("{}\n", std::process::id()));

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--quiet"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(
        root.join(".wt/A").exists(),
        "A should not be removed while live lock is held"
    );
    assert!(
        lock_path.exists(),
        "live process lock should remain in place"
    );
}

#[test]
fn test_clean_quiet_dead_lock_replaces_and_removes_lock_afterwards() {
    let repo = init_repo();
    let root = repo.path();

    create_merged_run(root, "A");

    // Spawn a short-lived process to get a guaranteed dead PID
    let mut child = Command::new("true").spawn().unwrap();
    let dead_pid = child.id();
    let _ = child.wait();

    let lock_path = root.join(".wt/clean.lock");
    write_file(&lock_path, &format!("{dead_pid}\n"));

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--quiet"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert!(
        !root.join(".wt/A").exists(),
        "A should be removed after replacing stale lock"
    );
    assert!(
        !lock_path.exists(),
        "clean.lock should be removed when clean ends"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("clean.lock"),
        "expected stderr to mention replacing stale lock, got: {stderr}"
    );
}

#[test]
fn test_clean_quiet_rejects_all_and_dry_run_and_hides_flags_in_help() {
    let repo = init_repo();
    let root = repo.path();

    for bad_args in [
        &["clean", "--quiet", "--all"][..],
        &["clean", "--quiet", "--dry-run"][..],
        &["clean", "--skip", "A", "--all"][..],
        &["clean", "--skip", "A", "--dry-run"][..],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
            .args(bad_args)
            .current_dir(root)
            .env("BLYTE_NO_UPDATE_CHECK", "1")
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "expected {:?} to be rejected",
            bad_args
        );
    }

    let help_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--help"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(help_out.status.success());
    let help_text = String::from_utf8_lossy(&help_out.stdout);
    assert!(
        !help_text.contains("--quiet"),
        "--quiet should be hidden from --help: {help_text}"
    );
    assert!(
        !help_text.contains("--skip"),
        "--skip should be hidden from --help: {help_text}"
    );
}

#[test]
fn test_review_accepted_removes_another_merged_run_in_background() {
    let repo = init_repo();
    let root = repo.path();

    // Another merged run M1 that should be cleaned by the background clean
    create_merged_run(root, "M1");

    // Unmerged run R1 being reviewed now
    let wt_r1 = root.join(".wt/R1");
    git(
        root,
        &["worktree", "add", "-b", "wip/R1", wt_r1.to_str().unwrap()],
    );
    write_file(&wt_r1.join("r1.txt"), "unmerged work\n");
    git(&wt_r1, &["add", "."]);
    git(&wt_r1, &["commit", "-qm", "feat: r1", "--no-gpg-sign"]);
    write_file(
        &root.join(".wt/R1.start.json"),
        &serde_json::json!({ "id": "R1", "pid": 999999 }).to_string(),
    );
    write_file(
        &root.join(".wt/R1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "R1", "--verdict", "accepted"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "review failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let wt_m1 = root.join(".wt/M1");
    let deadline = Instant::now() + Duration::from_secs(60);
    while wt_m1.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        !wt_m1.exists(),
        ".wt/M1 should be removed within 60 s by the background clean"
    );
    assert!(
        wt_r1.exists(),
        ".wt/R1 should still exist since it is not merged"
    );
}
