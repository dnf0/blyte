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
fn test_clean_verdict_accepted_unmerged_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T1");
    git(
        root,
        &["worktree", "add", "-b", "wip/T1", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("file.txt"), "some work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );
    write_file(&wt_dir.join("T1.log"), "log content\n");
    write_file(
        &wt_dir.join("T1.report.json"),
        &serde_json::json!({ "turns": 10 }).to_string(),
    );

    // Also place an unrelated directory in .wt to verify it is left alone
    write_file(&wt_dir.join("other_dir/some_file"), "leave me\n");

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
    assert!(stdout.contains("freed 0 B"), "stdout was: {stdout}");
    assert!(
        !stdout.contains("other_dir"),
        "stdout should not mention other_dir: {stdout}"
    );

    // Worktree directory kept
    assert!(wt_path.exists());
    // Record files stay
    assert!(wt_dir.join("T1.start.json").is_file());
    assert!(wt_dir.join("T1.json").is_file());
    assert!(wt_dir.join("T1.verdict.json").is_file());
    assert!(wt_dir.join("T1.log").is_file());
    assert!(wt_dir.join("T1.report.json").is_file());
    // Unrelated directory stays
    assert!(wt_dir.join("other_dir/some_file").is_file());

    // Branch wip/T1 stays
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success());
}

#[test]
fn test_clean_cherry_picked_onto_main_deletes_branch() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T2");
    git(
        root,
        &["worktree", "add", "-b", "wip/T2", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("cherry.txt"), "cherry content\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: cherry", "--no-gpg-sign"],
    );
    let commit_sha = git(&wt_path, &["rev-parse", "HEAD"]);

    // Cherry pick onto main
    git(root, &["cherry-pick", &commit_sha]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T2.start.json"),
        &serde_json::json!({ "id": "T2", "pid": 999999 }).to_string(),
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
        stdout.contains("removed .wt/T2 (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T2 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!wt_path.exists());
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T2"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!branch_check.status.success(), "branch should be deleted");
}

#[test]
fn test_clean_squash_merged_runs_removed_by_merge_tree_check() {
    let repo = init_repo();
    let root = repo.path();

    let s1_path = root.join(".wt/S1");
    git(
        root,
        &["worktree", "add", "-b", "wip/S1", s1_path.to_str().unwrap()],
    );
    write_file(&s1_path.join("s1.txt"), "feature 1\n");
    git(&s1_path, &["add", "."]);
    git(&s1_path, &["commit", "-qm", "feat: s1", "--no-gpg-sign"]);

    let s2_path = root.join(".wt/S2");
    git(
        root,
        &["worktree", "add", "-b", "wip/S2", s2_path.to_str().unwrap()],
    );
    write_file(&s2_path.join("s2.txt"), "feature 2\n");
    git(&s2_path, &["add", "."]);
    git(&s2_path, &["commit", "-qm", "feat: s2", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("S1.start.json"),
        &serde_json::json!({ "id": "S1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("S1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );
    write_file(
        &wt_dir.join("S2.start.json"),
        &serde_json::json!({ "id": "S2", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("S2.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    // Create the squash commit directly on main combining both changes
    write_file(&root.join("s1.txt"), "feature 1\n");
    write_file(&root.join("s2.txt"), "feature 2\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "squash: s1 and s2", "--no-gpg-sign"],
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/S1 (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/S1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/S2 (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/S2 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!s1_path.exists());
    assert!(!s2_path.exists());

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/S1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b1.status.success());
    let b2 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/S2"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!b2.status.success());
}

#[test]
fn test_clean_local_non_wip_branch_not_merged_kept() {
    let repo = init_repo();
    let root = repo.path();

    let f1_path = root.join(".wt/F1");
    git(
        root,
        &["worktree", "add", "-b", "wip/F1", f1_path.to_str().unwrap()],
    );

    write_file(&f1_path.join("feat.txt"), "feature\n");
    git(&f1_path, &["add", "."]);
    git(&f1_path, &["commit", "-qm", "feat: f1", "--no-gpg-sign"]);

    // Create a local non-wip branch containing wip/F1's tip, but main does not have it
    git(root, &["branch", "feature/my-feat", "wip/F1"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("F1.start.json"),
        &serde_json::json!({ "id": "F1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("F1.verdict.json"),
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
        stdout.contains("kept .wt/F1 (not merged yet)"),
        "stdout was: {stdout}"
    );
    assert!(f1_path.exists(), "worktree should be kept");

    let b1 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/F1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(b1.status.success(), "wip branch should be kept");
    let b2 = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/feature/my-feat"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(b2.status.success(), "feature branch should be kept");
}

#[test]
fn test_clean_verdict_redone_removes_worktree_keeps_unmerged_branch() {
    let repo = init_repo();
    let root = repo.path();

    let r1_path = root.join(".wt/R1");
    git(
        root,
        &["worktree", "add", "-b", "wip/R1", r1_path.to_str().unwrap()],
    );

    write_file(&r1_path.join("redone.txt"), "redone\n");
    git(&r1_path, &["add", "."]);
    git(&r1_path, &["commit", "-qm", "feat: r1", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("R1.start.json"),
        &serde_json::json!({ "id": "R1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("R1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("R1.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );
    write_file(&wt_dir.join("R1.log"), "log content\n");
    write_file(
        &wt_dir.join("R1.report.json"),
        &serde_json::json!({ "turns": 5 }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/R1 (verdict redone,"),
        "stdout was: {stdout}"
    );

    assert!(!r1_path.exists(), "worktree should be removed");
    assert!(wt_dir.join("R1.start.json").is_file());
    assert!(wt_dir.join("R1.json").is_file());
    assert!(wt_dir.join("R1.verdict.json").is_file());
    assert!(wt_dir.join("R1.log").is_file());
    assert!(wt_dir.join("R1.report.json").is_file());

    // Unmerged branch is kept
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/R1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        branch_check.status.success(),
        "unmerged branch wip/R1 must be kept"
    );
}

#[test]
fn test_clean_uncommitted_changes_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T3");
    git(
        root,
        &["worktree", "add", "-b", "wip/T3", wt_path.to_str().unwrap()],
    );

    // Leave an untracked uncommitted file in the worktree
    write_file(&wt_path.join("uncommitted.txt"), "not committed\n");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T3.start.json"),
        &serde_json::json!({ "id": "T3", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T3.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T3 (uncommitted changes)"),
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
fn test_clean_no_verdict_recent_kept_older_than_zero_removed() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T4");
    git(
        root,
        &["worktree", "add", "-b", "wip/T4", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("file.txt"), "work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T4.start.json"),
        &serde_json::json!({ "id": "T4", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T4.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    // 1. blyte clean with default older-than (14 days) should keep it because it finished recently
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("kept .wt/T4 (no verdict)"),
        "stdout was: {stdout}"
    );
    assert!(wt_path.exists(), "worktree should still exist");

    // 2. blyte clean --older-than 0 should remove it but keep its branch
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--older-than", "0"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("removed .wt/T4 (finished over 0 days ago,"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree should now be removed");

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T4"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        branch_check.status.success(),
        "unmerged branch wip/T4 must be kept"
    );
}

#[test]
fn test_clean_dry_run_changes_nothing() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T5");
    git(
        root,
        &["worktree", "add", "-b", "wip/T5", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("file.txt"), "work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T5.start.json"),
        &serde_json::json!({ "id": "T5", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T5.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--dry-run"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("would remove .wt/T5 (verdict redone,"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("would free "), "stdout was: {stdout}");

    assert!(wt_path.exists(), "worktree must not be removed in dry-run");
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T5"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success(), "branch must still exist");
}

#[test]
fn test_review_verdict_accepted_unmerged_worktree_kept() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T6");
    git(
        root,
        &["worktree", "add", "-b", "wip/T6", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("file.txt"), "work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T6.start.json"),
        &serde_json::json!({ "id": "T6", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T6.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "T6", "--verdict", "accepted"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "review failed: {stdout}");
    assert!(
        stdout.contains("recorded verdict accepted for T6"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("worktree .wt/T6 kept until wip/T6 is merged"),
        "stdout was: {stdout}"
    );
    assert!(!stdout.contains("removed"), "stdout was: {stdout}");

    assert!(wt_path.exists(), "worktree should be kept");
    assert!(
        wt_dir.join("T6.verdict.json").is_file(),
        "verdict file should exist"
    );

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T6"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success(), "branch should be kept");
}

#[test]
fn test_review_verdict_redone_removes_worktree_keeps_branch() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T7");
    git(
        root,
        &["worktree", "add", "-b", "wip/T7", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("file.txt"), "work\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: work", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T7.start.json"),
        &serde_json::json!({ "id": "T7", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T7.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "T7", "--verdict", "redone"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "review failed: {stdout}");
    assert!(
        stdout.contains("recorded verdict redone for T7"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T7; branch wip/T7 kept"),
        "stdout was: {stdout}"
    );

    assert!(!wt_path.exists(), "worktree should be removed");
    assert!(
        wt_dir.join("T7.verdict.json").is_file(),
        "verdict file should exist"
    );

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T7"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(branch_check.status.success(), "branch should be kept");
}

#[test]
fn test_review_verdict_accepted_merged_removes_worktree_deletes_branch() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/T8");
    git(
        root,
        &["worktree", "add", "-b", "wip/T8", wt_path.to_str().unwrap()],
    );

    write_file(&wt_path.join("merged.txt"), "merged content\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: merged", "--no-gpg-sign"],
    );
    let commit_sha = git(&wt_path, &["rev-parse", "HEAD"]);

    // Cherry pick onto main so work is merged
    git(root, &["cherry-pick", &commit_sha]);

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T8.start.json"),
        &serde_json::json!({ "id": "T8", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T8.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "T8", "--verdict", "accepted"])
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "review failed: {stdout}");
    assert!(
        stdout.contains("recorded verdict accepted for T8"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T8; branch wip/T8 deleted"),
        "stdout was: {stdout}"
    );

    assert!(!wt_path.exists(), "worktree should be removed");
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/T8"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!branch_check.status.success(), "branch should be deleted");
}

#[test]
fn test_clean_outcome_no_changes() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/NC1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/NC1",
            wt_path.to_str().unwrap(),
        ],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("NC1.start.json"),
        &serde_json::json!({ "id": "NC1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("NC1.json"),
        &serde_json::json!({ "outcome": "no_changes" }).to_string(),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("removed .wt/NC1 (no changes,"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree should be removed");
}

#[test]
fn test_clean_research_run() {
    let repo = init_repo();
    let root = repo.path();

    let wt_path = root.join(".wt/R1");
    std::fs::create_dir_all(&wt_path).unwrap();
    write_file(&wt_path.join("scratch.txt"), "some notes\n");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("R1.start.json"),
        &serde_json::json!({ "id": "R1", "pid": 999999, "research": true }).to_string(),
    );
    write_file(&wt_dir.join("R1.report.md"), "# Research Report\n");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("removed .wt/R1 (research report,"),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists(), "scratch directory should be removed");
}

#[test]
fn test_dispatch_cleans_quietly() {
    let repo = init_repo();
    let root = repo.path();

    // Create a finished worktree with a verdict that should be cleaned up by dispatch
    let wt_old = root.join(".wt/OLD");
    git(
        root,
        &["worktree", "add", "-b", "wip/OLD", wt_old.to_str().unwrap()],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("OLD.start.json"),
        &serde_json::json!({ "id": "OLD", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("OLD.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );

    write_file(&root.join("task.md"), "implement something\n");

    // Run dispatch for D1 with a non-existent or failing gate, but verify OLD was cleaned
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "D1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .current_dir(root)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("blyte: removed 1 finished worktrees"),
        "stderr was: {stderr}"
    );
    assert!(!wt_old.exists(), "old worktree should have been removed");
}
