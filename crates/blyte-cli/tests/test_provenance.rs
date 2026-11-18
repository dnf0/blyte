use blyte_cli::provenance::{render, source_state, SourceState};
use std::path::{Path, PathBuf};
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
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["commit", "--no-gpg-sign", "-q", "-m", msg]);
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(dir, &["add", "a.txt"]);
    commit(dir, "first");
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn repo_on_branch_without_origin_reports_uncommitted_and_head() {
    let tmp = tempdir();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    std::fs::write(repo.join("untracked.txt"), "x\n").unwrap();

    match source_state(&repo) {
        SourceState::Git {
            head,
            branch,
            uncommitted,
            default_ref,
            behind_ahead,
        } => {
            assert_eq!(head, git(&repo, &["rev-parse", "HEAD"]));
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(uncommitted, 1);
            assert_eq!(default_ref, None);
            assert_eq!(behind_ahead, None);
        }
        other => panic!("expected Git, got {other:?}"),
    }
}

#[test]
fn clone_behind_origin_reports_one_behind() {
    let tmp = tempdir();
    let origin = tmp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    init_repo(&origin);

    let clone = tmp.path().join("clone");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );

    std::fs::write(origin.join("b.txt"), "b\n").unwrap();
    git(&origin, &["add", "b.txt"]);
    commit(&origin, "second");
    git(&clone, &["fetch", "-q", "origin"]);

    match source_state(&clone) {
        SourceState::Git {
            default_ref,
            behind_ahead,
            uncommitted,
            ..
        } => {
            assert_eq!(default_ref.as_deref(), Some("origin/main"));
            assert_eq!(behind_ahead, Some((1, 0)));
            assert_eq!(uncommitted, 0);
        }
        other => panic!("expected Git, got {other:?}"),
    }
}

#[test]
fn detached_head_has_no_branch() {
    let tmp = tempdir();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    git(&repo, &["checkout", "-q", "--detach"]);

    match source_state(&repo) {
        SourceState::Git { branch, .. } => assert_eq!(branch, None),
        other => panic!("expected Git, got {other:?}"),
    }
}

#[test]
fn plain_directory_is_not_git() {
    let tmp = tempdir();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert_eq!(source_state(&plain), SourceState::NotGit);
}

#[test]
fn render_covers_each_variant() {
    let sources: Vec<(PathBuf, SourceState)> = vec![
        (
            PathBuf::from("/abs/repo"),
            SourceState::Git {
                head: "0123456789abcdef0123456789abcdef01234567".into(),
                branch: Some("feat/x".into()),
                uncommitted: 2,
                default_ref: Some("origin/main".into()),
                behind_ahead: Some((3, 1)),
            },
        ),
        (
            PathBuf::from("/abs/level"),
            SourceState::Git {
                head: "aaaa".into(),
                branch: Some("main".into()),
                uncommitted: 0,
                default_ref: Some("origin/main".into()),
                behind_ahead: Some((0, 0)),
            },
        ),
        (
            PathBuf::from("/abs/detached"),
            SourceState::Git {
                head: "bbbb".into(),
                branch: None,
                uncommitted: 0,
                default_ref: None,
                behind_ahead: None,
            },
        ),
        (PathBuf::from("/abs/cache"), SourceState::NotGit),
        (
            PathBuf::from("/abs/broken"),
            SourceState::Unknown("git status --porcelain: fatal: boom".into()),
        ),
    ];
    let text = render(&sources);
    assert!(text.starts_with("## Source state (written by the harness)"));
    for p in [
        "/abs/repo",
        "/abs/level",
        "/abs/detached",
        "/abs/cache",
        "/abs/broken",
    ] {
        assert!(text.contains(p), "missing path {p}");
    }
    assert!(text.contains("3 behind and 1 ahead of `origin/main`"));
    assert!(text.contains("2 uncommitted changes"));
    assert!(text.contains("level with `origin/main`"));
    assert!(text.contains("detached HEAD"));
    assert!(text.contains("not a git repository"));
    assert!(text.contains("unknown (git status --porcelain: fatal: boom)"));
    assert!(text.contains("Commit ids not listed here or shown in tool output must not be cited."));
}
