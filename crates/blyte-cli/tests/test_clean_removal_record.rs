use std::path::Path;
use std::process::Command;

use blyte_cli::removal_record::{self, RemovalRecord};

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

fn blyte_output(cwd: &Path, args: &[&str]) -> std::process::Output {
    let env_dir = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_STATE_DIR", env_dir.path().join("state"))
        .env("BLYTE_GATE_SLOT_DIR", env_dir.path().join("slots"))
        .env("HOME", env_dir.path().join("home"))
        .env("XDG_CONFIG_HOME", env_dir.path().join("home/.config"))
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn test_clean_merged_run_writes_removal_record_and_second_clean_preserves_bytes() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/M1");
    git(
        root,
        &["worktree", "add", "-b", "wip/M1", wt_path.to_str().unwrap()],
    );
    write_file(&wt_path.join("merged.txt"), "merged work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: M1", "--no-gpg-sign"]);
    git(root, &["merge", "--ff-only", "wip/M1"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("M1.start.json"),
        &serde_json::json!({ "id": "M1", "pid": 999999, "started_at": 1000 }).to_string(),
    );

    let out = blyte_output(root, &["clean"]);
    assert!(
        out.status.success(),
        "clean failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!wt_path.exists());
    assert!(!branch_exists(root, "wip/M1"));

    let record_path = removal_record::path(root, "M1");
    assert!(record_path.is_file());
    let first_bytes = std::fs::read(&record_path).unwrap();

    let record = removal_record::read(root, "M1")
        .unwrap()
        .expect("record should exist");
    assert_eq!(record.id, "M1");
    assert_eq!(record.reason, "merged");
    assert_eq!(record.detail, None);
    assert!(record.removed_at > 0);
    assert!(removal_record::is_merged(&record));

    let second = blyte_output(root, &["clean"]);
    assert!(
        second.status.success(),
        "second clean failed: stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second_bytes = std::fs::read(&record_path).unwrap();
    assert_eq!(first_bytes, second_bytes);
}

#[test]
fn test_clean_redone_verdict_writes_verdict_removal_record() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/R1");
    git(
        root,
        &["worktree", "add", "-b", "wip/R1", wt_path.to_str().unwrap()],
    );
    write_file(&wt_path.join("work.txt"), "redone work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: R1", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("R1.start.json"),
        &serde_json::json!({ "id": "R1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("R1.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );

    let out = blyte_output(root, &["clean"]);
    assert!(
        out.status.success(),
        "clean failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!wt_path.exists());
    assert!(branch_exists(root, "wip/R1"));

    let record = removal_record::read(root, "R1")
        .unwrap()
        .expect("record should exist");
    assert_eq!(record.id, "R1");
    assert_eq!(record.reason, "verdict");
    assert_eq!(record.detail.as_deref(), Some("redone"));
    assert!(record.removed_at > 0);
    assert!(!removal_record::is_merged(&record));
}

#[test]
fn test_clean_dry_run_writes_no_removal_record() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/D1");
    git(
        root,
        &["worktree", "add", "-b", "wip/D1", wt_path.to_str().unwrap()],
    );
    write_file(&wt_path.join("work.txt"), "merged work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: D1", "--no-gpg-sign"]);
    git(root, &["merge", "--ff-only", "wip/D1"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("D1.start.json"),
        &serde_json::json!({ "id": "D1", "pid": 999999, "started_at": 1000 }).to_string(),
    );

    let out = blyte_output(root, &["clean", "--dry-run"]);
    assert!(out.status.success());
    assert!(wt_path.exists());
    assert!(!removal_record::path(root, "D1").exists());
}

#[test]
fn test_removal_record_read_missing_invalid_and_unknown_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    assert_eq!(removal_record::read(root, "MISSING").unwrap(), None);

    let bad_path = removal_record::path(root, "BADJSON");
    write_file(&bad_path, "{");
    let err = removal_record::read(root, "BADJSON").unwrap_err();
    assert!(
        err.contains(&bad_path.display().to_string()),
        "error should name path {}: {err}",
        bad_path.display()
    );

    let unknown_path = removal_record::path(root, "BADREASON");
    let bad_record = RemovalRecord {
        id: "BADREASON".to_string(),
        reason: "bogus_reason".to_string(),
        detail: None,
        removed_at: 1795000000,
    };
    write_file(&unknown_path, &serde_json::to_string(&bad_record).unwrap());
    let err = removal_record::read(root, "BADREASON").unwrap_err();
    assert!(
        err.contains(&unknown_path.display().to_string()),
        "error should name path {}: {err}",
        unknown_path.display()
    );
    assert!(
        err.contains("bogus_reason"),
        "error should mention unknown reason: {err}"
    );
}

#[test]
fn test_clean_corrupt_verdict_file_exits_nonzero_and_keeps_run() {
    let repo = init_repo();
    let root = repo.path();

    let wt_bad = root.join(".wt/BAD1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/BAD1",
            wt_bad.to_str().unwrap(),
        ],
    );
    write_file(&wt_bad.join("bad.txt"), "work\n");
    git(&wt_bad, &["add", "."]);
    git(&wt_bad, &["commit", "-qm", "feat: BAD1", "--no-gpg-sign"]);

    let wt_good = root.join(".wt/GOOD1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/GOOD1",
            wt_good.to_str().unwrap(),
        ],
    );
    write_file(&wt_good.join("good.txt"), "good work\n");
    git(&wt_good, &["add", "."]);
    git(&wt_good, &["commit", "-qm", "feat: GOOD1", "--no-gpg-sign"]);
    git(root, &["merge", "--ff-only", "wip/GOOD1"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("BAD1.start.json"),
        &serde_json::json!({ "id": "BAD1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    let verdict_path = wt_dir.join("BAD1.verdict.json");
    write_file(&verdict_path, "not json");

    write_file(
        &wt_dir.join("GOOD1.start.json"),
        &serde_json::json!({ "id": "GOOD1", "pid": 999999, "started_at": 1000 }).to_string(),
    );

    let out = blyte_output(root, &["clean"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "clean should exit non-zero on corrupt verdict; stdout={} stderr={stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("BAD1.verdict.json"),
        "stderr should name the corrupt verdict file: {stderr}"
    );
    assert!(wt_bad.exists(), "BAD1 worktree must still exist");
    assert!(
        branch_exists(root, "wip/BAD1"),
        "wip/BAD1 branch must still exist"
    );
    assert!(!removal_record::path(root, "BAD1").exists());

    assert!(
        !wt_good.exists(),
        "GOOD1 should still be removed when another run fails"
    );
    assert!(!branch_exists(root, "wip/GOOD1"));
    assert!(removal_record::path(root, "GOOD1").is_file());
}

#[test]
fn test_clean_corrupt_child_json_exits_nonzero_and_keeps_parent() {
    let repo = init_repo();
    let root = repo.path();

    let wt_parent = root.join(".wt/P1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/P1",
            wt_parent.to_str().unwrap(),
        ],
    );
    write_file(&wt_parent.join("p1.txt"), "parent work\n");
    git(&wt_parent, &["add", "."]);
    git(&wt_parent, &["commit", "-qm", "feat: P1", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("P1.start.json"),
        &serde_json::json!({ "id": "P1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("P1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );
    write_file(
        &wt_dir.join("C1.start.json"),
        &serde_json::json!({
            "id": "C1",
            "pid": 999999,
            "started_at": 2000,
            "base": "wip/P1"
        })
        .to_string(),
    );
    write_file(&wt_dir.join("C1.json"), "not json");

    let out = blyte_output(root, &["clean"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "clean should exit non-zero on corrupt child summary; stdout={} stderr={stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("C1.json"),
        "stderr should name corrupt child file: {stderr}"
    );
    assert!(wt_parent.exists(), "P1 worktree must still exist");
    assert!(
        branch_exists(root, "wip/P1"),
        "wip/P1 branch must still exist"
    );
}

#[test]
fn test_clean_corrupt_child_verdict_exits_nonzero_and_keeps_parent() {
    let repo = init_repo();
    let root = repo.path();

    let wt_parent = root.join(".wt/P2");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/P2",
            wt_parent.to_str().unwrap(),
        ],
    );
    write_file(&wt_parent.join("p2.txt"), "parent work\n");
    git(&wt_parent, &["add", "."]);
    git(&wt_parent, &["commit", "-qm", "feat: P2", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("P2.start.json"),
        &serde_json::json!({ "id": "P2", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("P2.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );
    write_file(
        &wt_dir.join("C2.start.json"),
        &serde_json::json!({
            "id": "C2",
            "pid": 999999,
            "started_at": 2000,
            "base": "wip/P2"
        })
        .to_string(),
    );
    write_file(&wt_dir.join("C2.verdict.json"), "not json");

    let out = blyte_output(root, &["clean"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "clean should exit non-zero on corrupt child verdict; stdout={} stderr={stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("C2.verdict.json"),
        "stderr should name corrupt child verdict file: {stderr}"
    );
    assert!(wt_parent.exists(), "P2 worktree must still exist");
    assert!(
        branch_exists(root, "wip/P2"),
        "wip/P2 branch must still exist"
    );
}

#[test]
fn test_clean_record_write_failure_keeps_run_and_continues_others() {
    let repo = init_repo();
    let root = repo.path();

    let wt_fail = root.join(".wt/FAIL1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/FAIL1",
            wt_fail.to_str().unwrap(),
        ],
    );
    write_file(&wt_fail.join("fail.txt"), "fail work\n");
    git(&wt_fail, &["add", "."]);
    git(&wt_fail, &["commit", "-qm", "feat: FAIL1", "--no-gpg-sign"]);
    git(root, &["merge", "--ff-only", "wip/FAIL1"]);

    let wt_ok = root.join(".wt/OK1");
    git(
        root,
        &["worktree", "add", "-b", "wip/OK1", wt_ok.to_str().unwrap()],
    );
    write_file(&wt_ok.join("ok.txt"), "ok work\n");
    git(&wt_ok, &["add", "."]);
    git(&wt_ok, &["commit", "-qm", "feat: OK1", "--no-gpg-sign"]);
    git(root, &["merge", "--ff-only", "wip/OK1"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("FAIL1.start.json"),
        &serde_json::json!({ "id": "FAIL1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("OK1.start.json"),
        &serde_json::json!({ "id": "OK1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    std::fs::create_dir_all(removal_record::path(root, "FAIL1")).unwrap();

    let out = blyte_output(root, &["clean"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "clean should exit non-zero when record write fails; stdout={} stderr={stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("FAIL1"),
        "stderr should mention FAIL1: {stderr}"
    );
    assert!(wt_fail.exists(), "FAIL1 worktree must not be removed");
    assert!(
        branch_exists(root, "wip/FAIL1"),
        "wip/FAIL1 branch must not be deleted"
    );

    assert!(!wt_ok.exists(), "OK1 should still be removed");
    assert!(!branch_exists(root, "wip/OK1"));
    assert!(removal_record::path(root, "OK1").is_file());
}

#[test]
fn test_remove_finished_run_writes_removal_record() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/RF1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/RF1",
            wt_path.to_str().unwrap(),
        ],
    );
    write_file(&wt_path.join("rf.txt"), "rf work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: RF1", "--no-gpg-sign"]);
    git(root, &["merge", "--ff-only", "wip/RF1"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("RF1.start.json"),
        &serde_json::json!({ "id": "RF1", "pid": 999999, "started_at": 1000 }).to_string(),
    );

    let removed = blyte_cli::clean::remove_finished_run(root, "RF1").unwrap();
    assert_eq!(removed.as_deref(), Some("merged"));
    assert!(!wt_path.exists());
    assert!(!branch_exists(root, "wip/RF1"));

    let record = removal_record::read(root, "RF1")
        .unwrap()
        .expect("removal record should exist");
    assert_eq!(record.id, "RF1");
    assert_eq!(record.reason, "merged");
    assert_eq!(record.detail, None);
    assert!(record.removed_at > 0);
}

#[test]
fn test_review_redone_writes_removal_record() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/RV1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/RV1",
            wt_path.to_str().unwrap(),
        ],
    );
    write_file(&wt_path.join("rv.txt"), "rv work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: RV1", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("RV1.start.json"),
        &serde_json::json!({ "id": "RV1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("RV1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = blyte_output(root, &["review", "RV1", "--verdict", "redone"]);
    assert!(
        out.status.success(),
        "review failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!wt_path.exists());

    let record = removal_record::read(root, "RV1")
        .unwrap()
        .expect("removal record should exist");
    assert_eq!(record.id, "RV1");
    assert_eq!(record.reason, "verdict");
    assert_eq!(record.detail.as_deref(), Some("redone"));
    assert!(record.removed_at > 0);
}
