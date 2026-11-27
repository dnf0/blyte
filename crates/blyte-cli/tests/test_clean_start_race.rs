use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::clean::{CleanContext, KeptReason};

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
    // A run whose branch has no commits of its own: an ancestor of main.
    git(
        t.path(),
        &["worktree", "add", "-q", "-b", "wip/T1", ".wt/T1"],
    );
    t
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn a_run_started_after_the_snapshot_is_kept_as_running() {
    let repo = init_repo();
    let root = repo.path();
    let mut ctx = CleanContext::new(root);
    // The start record appears only after the snapshot was taken.
    write_file(
        &root.join(".wt/T1.start.json"),
        &serde_json::json!({
            "id": "T1",
            "pid": std::process::id(),
            "started_at": now_secs(),
        })
        .to_string(),
    );
    assert_eq!(ctx.check_removable("T1", 14), Err(KeptReason::Running));
}

#[test]
fn an_empty_start_record_keeps_the_worktree() {
    let repo = init_repo();
    let root = repo.path();
    write_file(&root.join(".wt/T1.start.json"), "");
    assert!(matches!(
        CleanContext::new(root).check_removable("T1", 14),
        Err(KeptReason::UnreadableStartRecord(_))
    ));
}

#[test]
fn a_truncated_start_record_keeps_the_worktree() {
    let repo = init_repo();
    let root = repo.path();
    write_file(&root.join(".wt/T1.start.json"), "{\"id\":\"T1\",\"pid\":");
    assert!(matches!(
        CleanContext::new(root).check_removable("T1", 14),
        Err(KeptReason::UnreadableStartRecord(_))
    ));
    blyte_cli::clean::clean_repo_with_options(root, 14, false, None, true, 7, Some(Ok(vec![])));
    assert!(root.join(".wt/T1").exists());
    assert!(Command::new("git")
        .args(["rev-parse", "--verify", "wip/T1"])
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn a_finished_merged_run_is_still_removed() {
    let repo = init_repo();
    let root = repo.path();
    write_file(
        &root.join(".wt/T1.start.json"),
        &serde_json::json!({"id": "T1", "pid": 0, "started_at": now_secs()}).to_string(),
    );
    assert!(CleanContext::new(root).check_removable("T1", 14).is_ok());
}

#[test]
fn write_start_record_leaves_a_whole_file_and_no_temp_file() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("T1.start.json");
    let value = serde_json::json!({"id": "T1", "pid": 42, "started_at": 1});
    blyte_cli::dispatch::write_start_record(&path, &value).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(parsed, value);
    assert!(!t.path().join("T1.start.json.tmp").exists());
}
