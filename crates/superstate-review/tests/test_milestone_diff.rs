use std::fs;
use std::process::Command;
use superstate_review::diff::get_milestone_diff;
use tempfile::TempDir;

fn setup_repo() -> (TempDir, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "tester@example.com"])
        .current_dir(root)
        .output()
        .unwrap();

    fs::write(root.join("base.txt"), "base content\n").unwrap();
    Command::new("git")
        .args(["add", "base.txt"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial commit", "--no-gpg-sign"])
        .current_dir(root)
        .output()
        .unwrap();

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    let initial_hash = String::from_utf8(head.stdout).unwrap().trim().to_string();
    (temp, initial_hash)
}

#[test]
fn test_get_milestone_diff_committed_only() {
    let (temp, initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\ncommitted change\n").unwrap();
    Command::new("git")
        .args(["add", "base.txt"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "update base", "--no-gpg-sign"])
        .current_dir(root)
        .output()
        .unwrap();

    let diff = get_milestone_diff(root, Some(&initial_hash)).unwrap();
    assert!(
        diff.contains("+committed change"),
        "Diff should contain committed changes, got:\n{}",
        diff
    );
}

#[test]
fn test_get_milestone_diff_uncommitted_only() {
    let (temp, initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\nuncommitted change\n").unwrap();

    let diff = get_milestone_diff(root, Some(&initial_hash)).unwrap();
    assert!(
        diff.contains("+uncommitted change"),
        "Diff should contain uncommitted changes, got:\n{}",
        diff
    );
}

#[test]
fn test_get_milestone_diff_committed_and_uncommitted() {
    let (temp, initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\ncommitted change\n").unwrap();
    Command::new("git")
        .args(["add", "base.txt"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "update base", "--no-gpg-sign"])
        .current_dir(root)
        .output()
        .unwrap();

    fs::write(root.join("uncommitted.txt"), "uncommitted file\n").unwrap();

    let diff = get_milestone_diff(root, Some(&initial_hash)).unwrap();
    assert!(
        diff.contains("+committed change"),
        "Diff should contain committed changes, got:\n{}",
        diff
    );
    assert!(
        diff.contains("+uncommitted file"),
        "Diff should contain untracked uncommitted file, got:\n{}",
        diff
    );
}

#[test]
fn test_get_milestone_diff_none_fallback_to_head_minus_1() {
    let (temp, _initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\nsecond commit\n").unwrap();
    Command::new("git")
        .args(["add", "base.txt"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "second commit", "--no-gpg-sign"])
        .current_dir(root)
        .output()
        .unwrap();

    let diff = get_milestone_diff(root, None).unwrap();
    assert!(
        diff.contains("+second commit"),
        "Clean working tree with None should fall back to HEAD~1..HEAD diff, got:\n{}",
        diff
    );
}

#[test]
fn test_get_milestone_diff_empty_repo() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(root)
        .output()
        .unwrap();

    let diff = get_milestone_diff(root, None).unwrap();
    assert!(diff.is_empty(), "Empty repo should yield empty diff");
}
