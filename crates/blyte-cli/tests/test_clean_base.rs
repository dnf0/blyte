use std::path::Path;
use std::process::Command;

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
    // Create feat/x branch
    git(t.path(), &["branch", "feat/x"]);
    t
}

#[test]
fn test_clean_base_unmerged_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1",
            wt_path.to_str().unwrap(),
            "feat/x",
        ],
    );

    write_file(&wt_path.join("file.txt"), "some work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999, "base": "feat/x" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T1 (not merged yet)"),
        "stdout was: {stdout}"
    );

    assert!(wt_path.exists());
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success());
}

#[test]
fn test_clean_base_merged_into_base_removes_worktree_and_deletes_branch() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1",
            wt_path.to_str().unwrap(),
            "feat/x",
        ],
    );

    write_file(&wt_path.join("file.txt"), "some work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);
    let commit_sha = git(&wt_path, &["rev-parse", "HEAD"]);

    // Cherry-pick commit into feat/x (not main)
    git(root, &["checkout", "feat/x"]);
    git(root, &["cherry-pick", &commit_sha]);
    git(root, &["checkout", "main"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999, "base": "feat/x" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged into feat/x,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!wt_path.exists(), "worktree should be removed");
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        !branch_check.status.success(),
        "wip/T1 branch should be deleted"
    );

    // feat/x branch should remain
    let feat_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/feat/x"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(feat_check.status.success(), "feat/x branch should remain");
}

#[test]
fn test_clean_base_main_merged_only_into_feat_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T2");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T2",
            wt_path.to_str().unwrap(),
            "feat/x",
        ],
    );

    write_file(&wt_path.join("file.txt"), "some work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);
    let commit_sha = git(&wt_path, &["rev-parse", "HEAD"]);

    // Cherry-pick commit into feat/x only (not main)
    git(root, &["checkout", "feat/x"]);
    git(root, &["cherry-pick", &commit_sha]);
    git(root, &["checkout", "main"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T2.start.json"),
        &serde_json::json!({ "id": "T2", "pid": 999999, "base": "main" }).to_string(),
    );
    write_file(
        &wt_dir.join("T2.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T2.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T2 (not merged yet)"),
        "stdout was: {stdout}"
    );

    assert!(wt_path.exists(), "worktree should be kept");
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T2"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success(), "branch should be kept");
}

#[test]
fn test_clean_base_squash_merged_into_base() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/S1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/S1",
            wt_path.to_str().unwrap(),
            "feat/x",
        ],
    );

    write_file(&wt_path.join("file.txt"), "squash work\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: squash work", "--no-gpg-sign"],
    );

    // Squash merge into feat/x (not main)
    git(root, &["checkout", "feat/x"]);
    git(root, &["merge", "--squash", "wip/S1"]);
    git(root, &["commit", "-qm", "squash merge S1", "--no-gpg-sign"]);
    git(root, &["checkout", "main"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("S1.start.json"),
        &serde_json::json!({ "id": "S1", "pid": 999999, "base": "feat/x" }).to_string(),
    );
    write_file(
        &wt_dir.join("S1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("S1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/S1 (merged into feat/x,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/S1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists());
}

#[test]
fn test_clean_base_origin_prefix_resolves_local() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/O1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/O1",
            wt_path.to_str().unwrap(),
            "feat/x",
        ],
    );

    write_file(&wt_path.join("file.txt"), "origin prefix work\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: origin prefix work", "--no-gpg-sign"],
    );
    let commit_sha = git(&wt_path, &["rev-parse", "HEAD"]);

    // Cherry-pick commit into feat/x (local branch without origin/ prefix)
    git(root, &["checkout", "feat/x"]);
    git(root, &["cherry-pick", &commit_sha]);
    git(root, &["checkout", "main"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("O1.start.json"),
        &serde_json::json!({ "id": "O1", "pid": 999999, "base": "origin/feat/x" }).to_string(),
    );
    write_file(
        &wt_dir.join("O1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("O1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/O1 (merged into feat/x,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/O1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists());
}

#[test]
fn test_clean_base_missing_base_field_behaves_as_before() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T3");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T3",
            wt_path.to_str().unwrap(),
            "feat/x",
        ],
    );

    write_file(&wt_path.join("file.txt"), "some work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);
    let commit_sha = git(&wt_path, &["rev-parse", "HEAD"]);

    // Cherry-pick commit into feat/x only (not main)
    git(root, &["checkout", "feat/x"]);
    git(root, &["cherry-pick", &commit_sha]);
    git(root, &["checkout", "main"]);

    let wt_dir = root.join(".wt");
    // No "base" field in start.json
    write_file(
        &wt_dir.join("T3.start.json"),
        &serde_json::json!({ "id": "T3", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T3.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T3.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T3 (not merged yet)"),
        "stdout was: {stdout}"
    );

    assert!(wt_path.exists(), "worktree should be kept");
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T3"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success(), "branch should be kept");
}

#[test]
fn test_dispatch_records_base_in_start_file() {
    let repo = init_repo();
    let root = repo.path();
    write_file(&root.join("task.md"), "implement something\n");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "D1",
            "task.md",
            "--base",
            "feat/x",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .current_dir(root)
        .env("BLYTE_STATE_DIR", root.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();

    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let start_file = root.join(".wt/D1.start.json");
    assert!(start_file.is_file(), "start file should exist");
    let start_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&start_file).unwrap()).unwrap();
    assert_eq!(start_json["base"], "feat/x");
}
