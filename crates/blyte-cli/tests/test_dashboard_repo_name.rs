//! A dispatch in a linked worktree belongs to its repository on the dashboard (2026-10-01).

use std::path::Path;
use std::process::Command;

use blyte_cli::dashboard::repo_name;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn linked_worktrees_and_sibling_names_resolve_to_the_repository() {
    let t = tempfile::tempdir().unwrap();
    let main = t.path().join("alpha");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("f"), "a\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "i", "--no-gpg-sign"]);

    let sibling = t.path().join("alpha--feature");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            sibling.to_str().unwrap(),
        ],
    );
    let nested = main.join(".claude").join("worktrees").join("fix-x");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix-x",
            nested.to_str().unwrap(),
        ],
    );

    assert_eq!(repo_name(&main), "alpha");
    assert_eq!(repo_name(&sibling), "alpha");
    assert_eq!(repo_name(&nested), "alpha");

    // Not a repository: the folder's own name, without a --suffix.
    let plain = t.path().join("beta--copy");
    std::fs::create_dir(&plain).unwrap();
    assert_eq!(repo_name(&plain), "beta");
}
