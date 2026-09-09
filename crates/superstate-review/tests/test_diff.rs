use std::fs;
use std::process::Command;
use superstate_review::diff::get_git_diff;
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
