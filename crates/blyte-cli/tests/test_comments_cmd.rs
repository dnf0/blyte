use std::path::Path;
use std::process::{Command, Output};

fn init_repo_with_base(dir: &Path, filename: &str, content: &str) -> String {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir)
        .output()
        .unwrap();
    std::fs::write(dir.join(filename), content).unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial", "--no-gpg-sign"])
        .current_dir(dir)
        .output()
        .unwrap();
    let rev = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(rev.stdout).unwrap().trim().to_string()
}

fn run_comments(dir: &Path, base: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .current_dir(dir)
        .args(["comments", "--base", base])
        .output()
        .unwrap()
}

#[test]
fn test_1_no_change_exits_0_empty_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let sha = init_repo_with_base(temp.path(), "a.rs", "fn main() {}\n");

    let out = run_comments(temp.path(), &sha);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn test_2_committed_change_adding_comment_exits_1() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let sha = init_repo_with_base(dir, "a.rs", "fn main() {}\n");

    std::fs::write(dir.join("a.rs"), "fn main() {}\n// x\n").unwrap();
    Command::new("git")
        .args(["add", "a.rs"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add comment", "--no-gpg-sign"])
        .current_dir(dir)
        .output()
        .unwrap();

    let out = run_comments(dir, &sha);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("comments added (1)"), "stdout: {stdout}");
    assert!(stdout.contains("a.rs:2: // x"), "stdout: {stdout}");
}

#[test]
fn test_3_uncommitted_change_adding_comment_exits_1() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let sha = init_repo_with_base(dir, "a.rs", "fn main() {}\n");

    std::fs::write(dir.join("a.rs"), "fn main() {}\n// x\n").unwrap();

    let out = run_comments(dir, &sha);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("comments added (1)"), "stdout: {stdout}");
    assert!(stdout.contains("a.rs:2: // x"), "stdout: {stdout}");
}

#[test]
fn test_4_new_untracked_python_file_with_comment_exits_1() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let sha = init_repo_with_base(dir, "a.rs", "fn main() {}\n");

    std::fs::write(dir.join("b.py"), "x = 1\n# y\n").unwrap();

    let out = run_comments(dir, &sha);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("b.py:2: # y"), "stdout: {stdout}");
}

#[test]
fn test_5_doc_comments_directives_and_unsupported_files_exit_0() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let sha = init_repo_with_base(dir, "a.rs", "fn main() {}\n");

    std::fs::write(
        dir.join("a.rs"),
        "/// doc\nfn main() {\n    // SAFETY: ok\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("notes.md"), "// z\n").unwrap();

    let out = run_comments(dir, &sha);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn test_6_existing_comment_with_added_code_below_exits_0() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let sha = init_repo_with_base(dir, "a.rs", "// old\nfn main() {}\n");

    std::fs::write(dir.join("a.rs"), "// old\nfn main() {}\nfn extra() {}\n").unwrap();

    let out = run_comments(dir, &sha);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn test_7_nonexistent_base_revision_exits_2() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let _sha = init_repo_with_base(dir, "a.rs", "fn main() {}\n");

    let out = run_comments(dir, "nonexistent-revision-0000");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("blyte comments: git diff failed:"),
        "stderr: {stderr}"
    );
}
