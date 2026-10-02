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
    t
}

#[test]
fn test_clean_followup_merged_removes_both_and_deletes_branches() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Squash merge wip/T1b into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1b"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1b", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
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
        stdout.contains("removed .wt/T1b (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists(), "T1 worktree should be removed");
    assert!(!t1b_path.exists(), "T1b worktree should be removed");

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1.status.success(), "wip/T1 branch should be deleted");

    let b1b = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1b"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1b.status.success(), "wip/T1b branch should be deleted");
}

#[test]
fn test_clean_followup_three_deep_only_last_merged() {
    let repo = init_repo();
    let root = repo.path();

    // T1 with a commit
    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    // T1b with base: wip/T1, rewriting T1's change
    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // T1c with base: wip/T1b, rewriting T1b's change
    let t1c_path = root.join(".wt/T1c");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1c",
            t1c_path.to_str().unwrap(),
            "wip/T1b",
        ],
    );
    write_file(&t1c_path.join("file.txt"), "rewritten by T1c\n");
    git(&t1c_path, &["add", "."]);
    git(
        &t1c_path,
        &["commit", "-qm", "feat: rewritten by T1c", "--no-gpg-sign"],
    );

    // Only T1c squash-merged into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1c"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1c", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1c.start.json"),
        &serde_json::json!({ "id": "T1c", "pid": 999999, "base": "wip/T1b" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.verdict.json"),
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
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1b (merged through T1c,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1c (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1c (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(!t1b_path.exists());
    assert!(!t1c_path.exists());

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1.status.success(), "wip/T1 branch should be deleted");

    let b1b = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1b"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1b.status.success(), "wip/T1b branch should be deleted");

    let b1c = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1c"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1c.status.success(), "wip/T1c branch should be deleted");
}

#[test]
fn test_clean_followup_not_merged_keeps_t1() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Neither merged into main

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
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
    assert!(
        stdout.contains("kept .wt/T1b (not merged yet)"),
        "stdout was: {stdout}"
    );

    assert!(t1_path.exists(), "T1 worktree should be kept");
    assert!(t1b_path.exists(), "T1b worktree should be kept");

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(b1.status.success(), "wip/T1 branch should be kept");

    let b1b = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1b"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(b1b.status.success(), "wip/T1b branch should be kept");
}

#[test]
fn test_clean_followup_child_already_deleted_by_earlier_clean() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    // T1b has no worktree directory and no wip/T1b branch in git,
    // but has start file with base: wip/T1, and outcome verified
    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists(), "T1 worktree should be removed");

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1.status.success(), "wip/T1 branch should be deleted");
}

#[test]
fn test_clean_followup_child_already_deleted_verdict_accepted() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    // T1b branch missing, verdict accepted (or fixed)
    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
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
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(!t1_path.exists());
}

#[test]
fn test_review_followup_merged_removes_worktree_deletes_branch() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Squash merge wip/T1b into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1b"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1b", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "T1", "--verdict", "accepted"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "review failed: {stdout}");
    assert!(stdout.contains("removed .wt/T1; branch wip/T1 deleted"));
    assert!(!t1_path.exists());
    assert!(!Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn test_clean_followup_dry_run() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Squash merge wip/T1b into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1b"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1b", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--dry-run"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("would remove .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would delete branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would remove .wt/T1b (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would delete branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );

    assert!(t1_path.exists());
    assert!(t1b_path.exists());
    assert!(Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1b"])
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn test_clean_followup_first_child_unmerged_second_child_merged() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    // First child: T1b, unmerged
    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Second child: T1c, merged into main
    let t1c_path = root.join(".wt/T1c");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1c",
            t1c_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1c_path.join("file.txt"), "rewritten by T1c\n");
    git(&t1c_path, &["add", "."]);
    git(
        &t1c_path,
        &["commit", "-qm", "feat: rewritten by T1c", "--no-gpg-sign"],
    );

    // Squash merge wip/T1c into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1c"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1c", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1c.start.json"),
        &serde_json::json!({ "id": "T1c", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.verdict.json"),
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
        stdout.contains("removed .wt/T1 (merged through T1c,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/T1b (not merged yet)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1c (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1c (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(t1b_path.exists());
    assert!(!t1c_path.exists());
}

#[test]
fn test_clean_followup_without_base_fields_inferred_from_git() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    // T1b branched from wip/T1 (tip of wip/T1 is ancestor of wip/T1b)
    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Squash merge T1b into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1b"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1b", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    // Both start files have NO "base" field
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
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
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1b (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(!t1b_path.exists());

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1.status.success(), "wip/T1 branch should be deleted");

    let b1b = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1b"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1b.status.success(), "wip/T1b branch should be deleted");
}

#[test]
fn test_clean_followup_refs_heads_wip_base_syntax() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work by T1", "--no-gpg-sign"],
    );

    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/T1b",
            t1b_path.to_str().unwrap(),
            "wip/T1",
        ],
    );
    write_file(&t1b_path.join("file.txt"), "rewritten by T1b\n");
    git(&t1b_path, &["add", "."]);
    git(
        &t1b_path,
        &["commit", "-qm", "feat: rewritten by T1b", "--no-gpg-sign"],
    );

    // Squash merge wip/T1b into main
    git(root, &["checkout", "main"]);
    git(root, &["merge", "--squash", "wip/T1b"]);
    git(
        root,
        &["commit", "-qm", "squash merge T1b", "--no-gpg-sign"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    // base uses full ref refs/heads/wip/T1
    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "refs/heads/wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(stdout.contains("removed .wt/T1 (merged through T1b,"));
    assert!(stdout.contains("deleted branch wip/T1 (merged)"));
    assert!(!t1_path.exists());
}

#[test]
fn test_clean_followup_cycle_guard() {
    let repo = init_repo();
    let root = repo.path();

    let t1_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", t1_path.to_str().unwrap()],
    );
    write_file(&t1_path.join("file1.txt"), "work by T1\n");
    git(&t1_path, &["add", "."]);
    git(
        &t1_path,
        &["commit", "-qm", "feat: work 1", "--no-gpg-sign"],
    );

    let t2_path = root.join(".wt/T2");
    git(
        root,
        &["worktree", "add", "-b", "wip/T2", t2_path.to_str().unwrap()],
    );
    write_file(&t2_path.join("file2.txt"), "work by T2\n");
    git(&t2_path, &["add", "."]);
    git(
        &t2_path,
        &["commit", "-qm", "feat: work 2", "--no-gpg-sign"],
    );

    // Cycle: T1 has base wip/T2, T2 has base wip/T1. Neither is merged.
    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999, "base": "wip/T2" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T2.start.json"),
        &serde_json::json!({ "id": "T2", "pid": 999999, "base": "wip/T1" }).to_string(),
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
    assert!(stdout.contains("kept .wt/T1 (not merged yet)"));
    assert!(stdout.contains("kept .wt/T2 (not merged yet)"));
    assert!(t1_path.exists());
    assert!(t2_path.exists());
}
