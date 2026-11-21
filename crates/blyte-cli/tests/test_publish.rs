use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use blyte_cli::publish::{open_pull_request, push_branch};

static PATH_LOCK: RwLock<()> = RwLock::new(());

struct PathReadGuard {
    _lock: RwLockReadGuard<'static, ()>,
}

impl PathReadGuard {
    fn acquire() -> Self {
        Self {
            _lock: PATH_LOCK.read().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

struct PathWriteGuard {
    prev_path: Option<OsString>,
    _lock: RwLockWriteGuard<'static, ()>,
}

impl PathWriteGuard {
    fn set_to_bin_dir(bin_dir: &Path) -> Self {
        let lock = PATH_LOCK.write().unwrap_or_else(|e| e.into_inner());
        let prev_path = std::env::var_os("PATH");
        make_bin_dir_with_git(bin_dir);
        std::env::set_var("PATH", bin_dir.as_os_str());
        Self {
            prev_path,
            _lock: lock,
        }
    }
}

impl Drop for PathWriteGuard {
    fn drop(&mut self) {
        match &self.prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {}: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "git {} failed in {}: stdout={} stderr={}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn find_git_binary() -> PathBuf {
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("git");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("/usr/bin/git")
}

fn setup_repo_with_bare_origin(default_branch: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::create_dir_all(&seed).unwrap();

    run_git(&bare, &["init", "--bare", "-b", default_branch]);

    run_git(&seed, &["init", "-b", default_branch]);
    run_git(&seed, &["config", "user.email", "test@example.com"]);
    run_git(&seed, &["config", "user.name", "Test User"]);
    std::fs::write(seed.join("README.md"), "init\n").unwrap();
    run_git(&seed, &["add", "README.md"]);
    run_git(&seed, &["commit", "--no-gpg-sign", "-m", "initial commit"]);
    run_git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
    run_git(&seed, &["push", "-u", "origin", default_branch]);

    run_git(
        tmp.path(),
        &["clone", bare.to_str().unwrap(), work.to_str().unwrap()],
    );
    run_git(&work, &["config", "user.email", "test@example.com"]);
    run_git(&work, &["config", "user.name", "Test User"]);

    (tmp, bare, work)
}

fn write_executable(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

fn make_bin_dir_with_git(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let git_bin = find_git_binary();
    std::os::unix::fs::symlink(git_bin, dir.join("git")).unwrap();
}

#[test]
fn push_branch_pushes_work_branch_to_local_bare_origin_and_sets_upstream() {
    let _guard = PathReadGuard::acquire();
    let (_tmp, bare, work) = setup_repo_with_bare_origin("main");

    run_git(&work, &["checkout", "-b", "feat/publish"]);
    std::fs::write(work.join("feature.txt"), "hello\n").unwrap();
    run_git(&work, &["add", "feature.txt"]);
    run_git(&work, &["commit", "--no-gpg-sign", "-m", "add feature"]);
    let local_sha = run_git(&work, &["rev-parse", "HEAD"]);

    push_branch(&work, "feat/publish").unwrap();

    let remote_sha = run_git(&bare, &["rev-parse", "refs/heads/feat/publish"]);
    assert_eq!(remote_sha, local_sha);

    let tracked_remote = run_git(&work, &["config", "branch.feat/publish.remote"]);
    let tracked_merge = run_git(&work, &["config", "branch.feat/publish.merge"]);
    assert_eq!(tracked_remote, "origin");
    assert_eq!(tracked_merge, "refs/heads/feat/publish");
}

#[test]
fn push_branch_never_uses_force_on_diverged_remote() {
    let _guard = PathReadGuard::acquire();
    let (_tmp, bare, work) = setup_repo_with_bare_origin("main");

    run_git(&work, &["checkout", "-b", "feat/diverge"]);
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    run_git(&work, &["add", "a.txt"]);
    run_git(&work, &["commit", "--no-gpg-sign", "-m", "first"]);
    push_branch(&work, "feat/diverge").unwrap();
    let pushed_sha = run_git(&bare, &["rev-parse", "refs/heads/feat/diverge"]);

    run_git(&work, &["reset", "--hard", "HEAD~1"]);
    std::fs::write(work.join("b.txt"), "two\n").unwrap();
    run_git(&work, &["add", "b.txt"]);
    run_git(&work, &["commit", "--no-gpg-sign", "-m", "diverged"]);

    let err = push_branch(&work, "feat/diverge").unwrap_err();
    assert!(err.contains("git push"), "unexpected error: {err}");
    assert_eq!(
        run_git(&bare, &["rev-parse", "refs/heads/feat/diverge"]),
        pushed_sha
    );
}

#[test]
fn push_branch_refuses_default_branch_read_from_origin_head() {
    let _guard = PathReadGuard::acquire();
    let (_tmp, bare, work) = setup_repo_with_bare_origin("trunk");
    let before_sha = run_git(&bare, &["rev-parse", "refs/heads/trunk"]);

    std::fs::write(work.join("change.txt"), "nope\n").unwrap();
    run_git(&work, &["add", "change.txt"]);
    run_git(&work, &["commit", "--no-gpg-sign", "-m", "on default"]);

    let err = push_branch(&work, "trunk").unwrap_err();
    assert!(
        err.contains("default branch") && err.contains("trunk"),
        "unexpected error: {err}"
    );
    assert_eq!(
        run_git(&bare, &["rev-parse", "refs/heads/trunk"]),
        before_sha
    );
}

#[test]
fn push_branch_errors_on_missing_branch() {
    let _guard = PathReadGuard::acquire();
    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");

    let err = push_branch(&work, "no-such-branch").unwrap_err();
    assert!(
        err.contains("no-such-branch") && err.contains("does not exist"),
        "unexpected error: {err}"
    );
}

#[test]
fn push_branch_errors_on_missing_origin_remote() {
    let _guard = PathReadGuard::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    run_git(repo, &["init", "-b", "main"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "user.name", "Test User"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "--no-gpg-sign", "-m", "init"]);
    run_git(repo, &["checkout", "-b", "feat/local"]);

    let err = push_branch(repo, "feat/local").unwrap_err();
    assert!(err.contains("origin"), "unexpected error: {err}");
}

#[test]
fn push_branch_errors_on_unreadable_default_branch() {
    let _guard = PathReadGuard::acquire();
    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    run_git(&work, &["checkout", "-b", "feat/no-head"]);
    run_git(&work, &["remote", "set-head", "origin", "-d"]);

    let err = push_branch(&work, "feat/no-head").unwrap_err();
    assert!(err.contains("origin/HEAD"), "unexpected error: {err}");

    let sha = run_git(&work, &["rev-parse", "HEAD"]);
    run_git(&work, &["update-ref", "refs/remotes/origin/HEAD", &sha]);
    let detached_err = push_branch(&work, "feat/no-head").unwrap_err();
    assert!(
        detached_err.contains("origin/HEAD"),
        "unexpected error: {detached_err}"
    );
}

#[test]
fn open_pull_request_runs_gh_with_expected_args_and_returns_url() {
    let bin_tmp = tempfile::tempdir().unwrap();
    let bin_dir = bin_tmp.path().join("bin");
    let _guard = PathWriteGuard::set_to_bin_dir(&bin_dir);

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("trunk");
    run_git(&work, &["checkout", "-b", "feat/open-pr"]);

    let args_file = bin_tmp.path().join("gh-args.bin");
    let fake_gh = bin_dir.join("gh");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > '{}'\nprintf 'https://github.com/example/repo/pull/42\\n'\n",
        args_file.display()
    );
    write_executable(&fake_gh, &script);

    let title = "Add publish helpers ($(not-a-subshell))";
    let body = "- first line with `backticks`\n- second line with 'quotes' and \"double quotes\"";
    let url = open_pull_request(&work, "feat/open-pr", title, body).unwrap();
    assert_eq!(url, "https://github.com/example/repo/pull/42");

    let raw = std::fs::read(&args_file).unwrap();
    let mut parts: Vec<String> = raw
        .split(|&b| b == 0)
        .map(|slice| String::from_utf8(slice.to_vec()).unwrap())
        .collect();
    assert_eq!(parts.pop().as_deref(), Some(""));
    assert_eq!(
        parts,
        vec![
            "pr",
            "create",
            "--base",
            "trunk",
            "--head",
            "feat/open-pr",
            "--title",
            title,
            "--body",
            body,
        ]
    );
}

#[test]
fn open_pull_request_errors_when_gh_is_missing() {
    let bin_tmp = tempfile::tempdir().unwrap();
    let bin_dir = bin_tmp.path().join("bin");
    let _guard = PathWriteGuard::set_to_bin_dir(&bin_dir);

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    run_git(&work, &["checkout", "-b", "feat/missing-gh"]);

    let err = open_pull_request(&work, "feat/missing-gh", "Title", "Body").unwrap_err();
    assert!(
        err.contains("gh") && (err.contains("not installed") || err.contains("not found")),
        "unexpected error: {err}"
    );
}

#[test]
fn open_pull_request_errors_when_gh_is_not_signed_in() {
    let bin_tmp = tempfile::tempdir().unwrap();
    let bin_dir = bin_tmp.path().join("bin");
    let _guard = PathWriteGuard::set_to_bin_dir(&bin_dir);

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    run_git(&work, &["checkout", "-b", "feat/not-logged-in"]);

    let fake_gh = bin_dir.join("gh");
    write_executable(
        &fake_gh,
        "#!/bin/sh\necho 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n",
    );

    let err = open_pull_request(&work, "feat/not-logged-in", "Title", "Body").unwrap_err();
    assert!(
        err.contains("not signed in") && err.contains("gh"),
        "unexpected error: {err}"
    );
}

#[test]
fn open_pull_request_refuses_empty_title_and_default_branch_before_running_gh() {
    let bin_tmp = tempfile::tempdir().unwrap();
    let bin_dir = bin_tmp.path().join("bin");
    let _guard = PathWriteGuard::set_to_bin_dir(&bin_dir);

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    run_git(&work, &["checkout", "-b", "feat/work"]);

    let marker = bin_tmp.path().join("gh-ran");
    let fake_gh = bin_dir.join("gh");
    let script = format!("#!/bin/sh\n: > '{}'\nexit 0\n", marker.display());
    write_executable(&fake_gh, &script);

    let empty_err = open_pull_request(&work, "feat/work", "   \n\t ", "Body").unwrap_err();
    assert!(empty_err.contains("title"), "unexpected error: {empty_err}");
    assert!(!marker.exists(), "gh should not run when title is empty");

    let default_err = open_pull_request(&work, "main", "Valid title", "Body").unwrap_err();
    assert!(
        default_err.contains("default branch") && default_err.contains("main"),
        "unexpected error: {default_err}"
    );
    assert!(
        !marker.exists(),
        "gh should not run when branch is the default branch"
    );
}
