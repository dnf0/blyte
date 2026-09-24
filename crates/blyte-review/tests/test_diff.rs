use blyte_review::diff::get_git_diff;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_get_git_diff_in_repo() {
    let dir = tempdir().unwrap();
    let repo_path = dir.path();

    // Initialize git repo
    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let _ = Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let _ = Command::new("git")
        .args(["config", "user.email", "tester@test.com"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    fs::write(repo_path.join("file.txt"), "hello world\n").unwrap();
    let _ = Command::new("git")
        .args(["add", "file.txt"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "--no-gpg-sign", "-m", "initial commit"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    fs::write(repo_path.join("file.txt"), "hello world\nsecond line\n").unwrap();
    let diff = get_git_diff(repo_path, None).expect("Failed to get git diff");
    assert!(diff.contains("+second line"));
}

#[test]
fn test_get_git_diff_captures_untracked_files() {
    let dir = tempdir().unwrap();
    let repo_path = dir.path();

    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let _ = Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let _ = Command::new("git")
        .args(["config", "user.email", "tester@test.com"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    fs::write(repo_path.join("tracked.txt"), "initial\n").unwrap();
    let _ = Command::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "--no-gpg-sign", "-m", "initial commit"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    // Create an untracked file without staging it
    fs::write(repo_path.join("brand_new.txt"), "untracked content\n").unwrap();
    fs::write(repo_path.join("special name.txt"), "spaced content\n").unwrap();

    let diff = get_git_diff(repo_path, Some("HEAD")).expect("Failed to get git diff against HEAD");
    assert!(
        diff.contains("brand_new.txt"),
        "Diff against HEAD should include untracked file, got:\n{}",
        diff
    );
    assert!(diff.contains("+untracked content"));
    assert!(diff.contains("special name.txt"));

    // Verify commit_range = None also captures untracked files
    let diff_none = get_git_diff(repo_path, None).expect("Failed to get git diff with None");
    assert!(diff_none.contains("brand_new.txt"));

    // Verify index is untouched: git status still shows ?? (untracked)
    let status_out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&status_out.stdout);
    assert!(
        status.contains("?? brand_new.txt"),
        "Index must remain untracked after get_git_diff, got:\n{}",
        status
    );

    // Make another commit on tracked.txt
    fs::write(repo_path.join("tracked.txt"), "second commit\n").unwrap();
    let _ = Command::new("git")
        .args(["commit", "-am", "second commit", "--no-gpg-sign"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    // Explicit commit range must NOT include untracked working copy files
    let diff_range =
        get_git_diff(repo_path, Some("HEAD~1..HEAD")).expect("Failed to get git diff for range");
    assert!(
        !diff_range.contains("brand_new.txt"),
        "Explicit range must not include untracked working copy file"
    );
    assert!(diff_range.contains("+second commit"));
}
