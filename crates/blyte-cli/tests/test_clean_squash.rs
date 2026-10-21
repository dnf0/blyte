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
fn test_squash_merged() {
    let repo = init_repo();
    let root = repo.path();

    git(root, &["checkout", "-b", "feat/x"]);
    write_file(&root.join("a.txt"), "line 1\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "feat: add a", "--no-gpg-sign"]);

    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fix: tweak a", "--no-gpg-sign"]);

    git(root, &["checkout", "main"]);
    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: x (#1)\n\n* feat: add a\n\n* fix: tweak a",
            "--no-gpg-sign",
        ],
    );

    // Later commit on main that edits the same lines so merge-tree does not match
    write_file(&root.join("a.txt"), "line 1 edited later on main\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "chore: edit a later", "--no-gpg-sign"],
    );

    assert!(blyte_cli::clean::is_branch_merged(root, "feat/x"));
}

#[test]
fn test_partly_merged() {
    let repo = init_repo();
    let root = repo.path();

    git(root, &["checkout", "-b", "feat/x"]);
    write_file(&root.join("a.txt"), "line 1\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "feat: add a", "--no-gpg-sign"]);

    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fix: tweak a", "--no-gpg-sign"]);

    git(root, &["checkout", "main"]);
    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: x (#1)\n\n* feat: add a",
            "--no-gpg-sign",
        ],
    );

    write_file(&root.join("a.txt"), "line 1 edited later on main\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "chore: edit a later", "--no-gpg-sign"],
    );

    assert!(!blyte_cli::clean::is_branch_merged(root, "feat/x"));
}

#[test]
fn test_old_subject_only() {
    let repo = init_repo();
    let root = repo.path();

    // Commit "fix: typo" on main before the branch forks
    write_file(&root.join("README.md"), "initial fixed\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fix: typo", "--no-gpg-sign"]);

    git(root, &["checkout", "-b", "feat/x"]);
    write_file(&root.join("other.txt"), "different content\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fix: typo", "--no-gpg-sign"]);

    git(root, &["checkout", "main"]);

    assert!(!blyte_cli::clean::is_branch_merged(root, "feat/x"));
}

#[test]
fn test_substring_only() {
    let repo = init_repo();
    let root = repo.path();

    git(root, &["checkout", "-b", "feat/x"]);
    write_file(&root.join("a.txt"), "line 1\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "feat: add a", "--no-gpg-sign"]);

    git(root, &["checkout", "main"]);
    write_file(&root.join("a.txt"), "line 1 and more\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: x (#1)\n\n* feat: add a and more",
            "--no-gpg-sign",
        ],
    );

    assert!(!blyte_cli::clean::is_branch_merged(root, "feat/x"));
}
