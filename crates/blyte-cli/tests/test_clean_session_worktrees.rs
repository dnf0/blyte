use std::path::{Path, PathBuf};
use std::process::Command;

use blyte_cli::clean::{
    check_session_worktree_removable, clean_repo_with_options, is_session_worktree,
    parse_lsof_cwd_output, parse_worktree_list, KeptReason, RemovalReason,
};

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

fn init_repo_with_parent() -> (tempfile::TempDir, PathBuf) {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("my-repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    write_file(&root.join("README.md"), "initial\n");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial", "--no-gpg-sign"]);
    (outer, root)
}

fn set_file_age_days(path: &Path, days: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as libc::time_t;
    let target = now - (days as libc::time_t * 86400);
    let times = [
        libc::timeval {
            tv_sec: target,
            tv_usec: 0,
        },
        libc::timeval {
            tv_sec: target,
            tv_usec: 0,
        },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let rc = unsafe { libc::utimes(c_path.as_ptr(), times.as_ptr()) };
    assert_eq!(rc, 0, "utimes failed on {}", path.display());
}

fn backdate_worktree_admin_files(wt_path: &Path, index_days: u64, head_days: u64) {
    let git_dir = PathBuf::from(git(wt_path, &["rev-parse", "--absolute-git-dir"]));
    set_file_age_days(&git_dir.join("index"), index_days);
    set_file_age_days(&git_dir.join("HEAD"), head_days);
}

#[test]
fn test_session_worktree_wt_merged_removed_and_branch_deleted() {
    let (_outer, root) = init_repo_with_parent();

    let wt_path = root.join(".wt").join("plan-AC");
    git(
        &root,
        &["worktree", "add", "-b", "fix/x", wt_path.to_str().unwrap()],
    );
    write_file(&wt_path.join("feature.txt"), "done\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "fix: x", "--no-gpg-sign"]);
    git(&root, &["merge", "--ff-only", "fix/x"]);

    backdate_worktree_admin_files(&wt_path, 8, 8);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "clean failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("removed .wt/plan-AC (fix/x merged, "),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch fix/x (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        !stdout.contains("merged worktree .wt/plan-AC"),
        "session worktree must not be reported by report_other_worktrees: {stdout}"
    );
    assert!(
        !stdout.contains("freed 0 B"),
        "freed bytes should include the removed session worktree: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree should have been removed");

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/fix/x"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !branch_check.status.success(),
        "branch fix/x should have been deleted"
    );
}

#[test]
fn test_session_worktree_claude_worktrees_merged_and_removed() {
    let (outer, root) = init_repo_with_parent();

    let claude_wt = root.join(".claude").join("worktrees").join("x");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/claude-x",
            claude_wt.to_str().unwrap(),
        ],
    );
    write_file(&claude_wt.join("claude.txt"), "claude session work\n");
    git(&claude_wt, &["add", "."]);
    git(
        &claude_wt,
        &["commit", "-qm", "feat: claude work", "--no-gpg-sign"],
    );
    git(&root, &["merge", "--ff-only", "feat/claude-x"]);

    // Also add an outside sibling worktree to verify report_other_worktrees still reports outside ones
    // while omitting .claude/worktrees/x
    let side_path = outer.path().join("my-repo--side");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "side-branch",
            side_path.to_str().unwrap(),
        ],
    );

    backdate_worktree_admin_files(&claude_wt, 8, 8);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .claude/worktrees/x (feat/claude-x merged, "),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch feat/claude-x (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        !stdout.contains("merged worktree .claude/worktrees/x"),
        ".claude/worktrees/x must not be listed in report_other_worktrees: {stdout}"
    );
    assert!(
        stdout.contains("merged worktree ../my-repo--side (side-branch,"),
        "outside worktree must still be reported: {stdout}"
    );
    assert!(!claude_wt.exists(), ".claude/worktrees/x should be removed");
    assert!(side_path.exists(), "outside worktree must not be removed");
}

#[test]
fn test_session_worktree_squash_merged_branch_removed() {
    let (_outer, root) = init_repo_with_parent();

    let wt_path = root.join(".wt").join("plan-SQ");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/squash",
            wt_path.to_str().unwrap(),
        ],
    );
    write_file(&wt_path.join("sq1.txt"), "part 1\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: squash part 1", "--no-gpg-sign"],
    );
    write_file(&wt_path.join("sq2.txt"), "part 2\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: squash part 2", "--no-gpg-sign"],
    );

    // Advance main with another commit first so it's not a trivial ancestor
    write_file(&root.join("other.txt"), "other\n");
    git(&root, &["add", "."]);
    git(
        &root,
        &["commit", "-qm", "chore: other on main", "--no-gpg-sign"],
    );

    // Squash merge feat/squash onto main with GitHub-style commit message
    git(&root, &["merge", "--squash", "feat/squash"]);
    git(
        &root,
        &[
            "commit",
            "-qm",
            "feat: squash (#42)\n\n* feat: squash part 1\n* feat: squash part 2",
            "--no-gpg-sign",
        ],
    );

    let stats = clean_repo_with_options(&root, 14, false, None, false, 0, Some(Ok(vec![])));
    assert_eq!(stats.removed_worktrees, 1);
    assert!(stats.freed_bytes > 0);
    assert!(!stats.had_errors);
    assert!(!wt_path.exists());

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/feat/squash"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!branch_check.status.success());
}

#[test]
fn test_session_worktree_detached_head_merged_removed() {
    let (_outer, root) = init_repo_with_parent();

    let head_sha = git(&root, &["rev-parse", "HEAD"]);
    let short_sha = &head_sha[..7];

    let wt_path = root.join(".wt").join("plan-DET");
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            wt_path.to_str().unwrap(),
            "HEAD",
        ],
    );

    backdate_worktree_admin_files(&wt_path, 8, 8);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains(&format!("removed .wt/plan-DET ({short_sha} merged, ")),
        "stdout was: {stdout}"
    );
    assert!(!wt_path.exists());
}

#[test]
fn test_session_worktree_kept_reasons() {
    let (_outer, root) = init_repo_with_parent();

    // 1. Not merged yet: .wt/plan-RB
    let wt_unmerged = root.join(".wt").join("plan-RB");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/unmerged",
            wt_unmerged.to_str().unwrap(),
        ],
    );
    write_file(&wt_unmerged.join("unmerged.txt"), "wip\n");
    git(&wt_unmerged, &["add", "."]);
    git(
        &wt_unmerged,
        &["commit", "-qm", "feat: wip", "--no-gpg-sign"],
    );
    backdate_worktree_admin_files(&wt_unmerged, 8, 8);

    // 2. Uncommitted changes: .wt/plan-DIRTY
    let wt_dirty = root.join(".wt").join("plan-DIRTY");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/dirty",
            wt_dirty.to_str().unwrap(),
        ],
    );
    write_file(&wt_dirty.join("dirty.txt"), "uncommitted\n");
    backdate_worktree_admin_files(&wt_dirty, 8, 8);

    // 3. Holds runs: .wt/plan-RUNS
    let wt_runs = root.join(".wt").join("plan-RUNS");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/runs",
            wt_runs.to_str().unwrap(),
        ],
    );
    write_file(
        &wt_runs.join(".wt").join("WT1.start.json"),
        r#"{"id":"WT1","pid":999999}"#,
    );
    backdate_worktree_admin_files(&wt_runs, 8, 8);

    // 4. Used in the last 7 days: .wt/plan-RECENT (admin files are fresh)
    let wt_recent = root.join(".wt").join("plan-RECENT");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/recent",
            wt_recent.to_str().unwrap(),
        ],
    );

    // 5. In use by a running process: .wt/plan-BUSY (spawn a child process with cwd in .wt/plan-BUSY/sub)
    let wt_busy = root.join(".wt").join("plan-BUSY");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/busy",
            wt_busy.to_str().unwrap(),
        ],
    );
    let busy_sub = wt_busy.join("nested").join("dir");
    std::fs::create_dir_all(&busy_sub).unwrap();
    backdate_worktree_admin_files(&wt_busy, 8, 8);

    let mut sleeper = Command::new("sleep")
        .arg("30")
        .current_dir(&busy_sub)
        .spawn()
        .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let _ = sleeper.kill();
    let _ = sleeper.wait();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/plan-RB (not merged yet)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/plan-DIRTY (uncommitted changes)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/plan-RUNS (holds runs: review them from .wt/plan-RUNS)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/plan-RECENT (used in the last 7 days)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/plan-BUSY (in use by a running process)"),
        "stdout was: {stdout}"
    );
    assert!(
        !stdout.contains("merged worktree"),
        "none of the session worktrees should be reported in report_other_worktrees: {stdout}"
    );

    assert!(wt_unmerged.exists());
    assert!(wt_dirty.exists());
    assert!(wt_runs.exists());
    assert!(wt_recent.exists());
    assert!(wt_busy.exists());
}

#[test]
fn test_session_worktree_idle_checks_both_index_and_head() {
    let (_outer, root) = init_repo_with_parent();

    let wt_path = root.join(".wt").join("plan-IDLE");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/idle",
            wt_path.to_str().unwrap(),
        ],
    );

    let wts = parse_worktree_list(&root);
    let wt_info = wts
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/idle"))
        .unwrap();

    // Only index is old, HEAD is fresh -> kept
    backdate_worktree_admin_files(&wt_path, 8, 1);
    assert_eq!(
        check_session_worktree_removable(&root, wt_info, &Ok(vec![]), 7),
        Err(KeptReason::UsedRecently(7))
    );

    // Only HEAD is old, index is fresh -> kept
    backdate_worktree_admin_files(&wt_path, 1, 8);
    assert_eq!(
        check_session_worktree_removable(&root, wt_info, &Ok(vec![]), 7),
        Err(KeptReason::UsedRecently(7))
    );

    // Both index and HEAD are > 7 days old -> removable
    backdate_worktree_admin_files(&wt_path, 8, 8);
    assert_eq!(
        check_session_worktree_removable(&root, wt_info, &Ok(vec![]), 7),
        Ok(RemovalReason::SessionMerged("feat/idle".to_string()))
    );
}

#[test]
fn test_session_worktree_lsof_error_keeps_worktree() {
    let (_outer, root) = init_repo_with_parent();

    let wt_path = root.join(".wt").join("plan-LSOF");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/lsof",
            wt_path.to_str().unwrap(),
        ],
    );

    let wts = parse_worktree_list(&root);
    let wt_info = wts
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/lsof"))
        .unwrap();

    let cwds_err = Err("No such file or directory (os error 2)".to_string());
    let res = check_session_worktree_removable(&root, wt_info, &cwds_err, 0);
    assert_eq!(
        res,
        Err(KeptReason::CannotCheckInUse(
            "No such file or directory (os error 2)".to_string()
        ))
    );
    assert_eq!(
        res.unwrap_err().to_string(),
        "cannot tell whether it is in use: No such file or directory (os error 2)"
    );

    let stats = clean_repo_with_options(&root, 14, false, None, false, 0, Some(cwds_err));
    assert_eq!(stats.removed_worktrees, 0);
    assert!(wt_path.exists());
}

#[test]
fn test_session_worktree_dry_run() {
    let (_outer, root) = init_repo_with_parent();

    let wt_path = root.join(".wt").join("plan-DRY");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/dry",
            wt_path.to_str().unwrap(),
        ],
    );
    write_file(&wt_path.join("dry.txt"), "dry run content\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: dry", "--no-gpg-sign"]);
    git(&root, &["merge", "--ff-only", "feat/dry"]);

    backdate_worktree_admin_files(&wt_path, 8, 8);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--dry-run"])
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean --dry-run failed: {stdout}");
    assert!(
        stdout.contains("would remove .wt/plan-DRY (feat/dry merged, "),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would delete branch feat/dry (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        !stdout.contains("would free 0 B"),
        "would free should count session worktree bytes: {stdout}"
    );
    assert!(wt_path.exists(), "dry-run must not remove worktree");

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/feat/dry"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        branch_check.status.success(),
        "dry-run must not delete branch"
    );
}

#[test]
fn test_session_worktree_branch_kept_when_checked_out_in_another_worktree() {
    let (outer, root) = init_repo_with_parent();

    let wt_path = root.join(".wt").join("plan-SHARED");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feat/shared",
            wt_path.to_str().unwrap(),
        ],
    );

    // Check out the same branch in an outside worktree using --force
    let other_wt = outer.path().join("my-repo--shared");
    git(
        &root,
        &[
            "worktree",
            "add",
            "--force",
            other_wt.to_str().unwrap(),
            "feat/shared",
        ],
    );

    let stats = clean_repo_with_options(&root, 14, false, None, false, 0, Some(Ok(vec![])));
    assert_eq!(stats.removed_worktrees, 1);
    assert!(!wt_path.exists(), "session worktree should be removed");

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/feat/shared"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch feat/shared must not be deleted while checked out in another worktree"
    );
}

#[test]
fn test_session_worktree_remove_failure_prints_error_continues_and_exits_nonzero() {
    let (_outer, root) = init_repo_with_parent();

    let wt_a = root.join(".wt").join("plan-A");
    git(
        &root,
        &["worktree", "add", "-b", "feat/a", wt_a.to_str().unwrap()],
    );
    backdate_worktree_admin_files(&wt_a, 8, 8);
    // Lock plan-A so `git worktree remove` (without --force) fails
    git(&root, &["worktree", "lock", wt_a.to_str().unwrap()]);

    let wt_b = root.join(".wt").join("plan-B");
    git(
        &root,
        &["worktree", "add", "-b", "feat/b", wt_b.to_str().unwrap()],
    );
    backdate_worktree_admin_files(&wt_b, 8, 8);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "clean must exit non-zero when git worktree remove fails\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains(".wt/plan-A") && stderr.contains("locked"),
        "stderr should name .wt/plan-A and git's message: {stderr}"
    );
    assert!(
        stdout.contains("removed .wt/plan-B (feat/b merged, "),
        "clean must keep going and remove .wt/plan-B: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch feat/b (merged)"),
        "clean must delete feat/b: {stdout}"
    );
    assert!(wt_a.exists(), "locked worktree .wt/plan-A should remain");
    assert!(!wt_b.exists(), ".wt/plan-B should be removed");
}

#[test]
fn test_parse_lsof_cwd_output_and_is_session_worktree() {
    let sample = "p123\nfcwd\nn/tmp/foo\np456\nfcwd\nn/tmp/bar/baz\n";
    let parsed = parse_lsof_cwd_output(sample);
    assert_eq!(
        parsed,
        vec![PathBuf::from("/tmp/foo"), PathBuf::from("/tmp/bar/baz")]
    );

    let (_outer, root) = init_repo_with_parent();
    let run_wt = root.join(".wt").join("RUN1");
    std::fs::create_dir_all(&run_wt).unwrap();
    write_file(&root.join(".wt").join("RUN1.start.json"), "{}");

    let session_wt = root.join(".wt").join("plan-WT");
    std::fs::create_dir_all(&session_wt).unwrap();

    let claude_wt = root.join(".claude").join("worktrees").join("session-1");
    std::fs::create_dir_all(&claude_wt).unwrap();

    assert!(
        !is_session_worktree(&root, &run_wt),
        "run with .start.json is not a session worktree"
    );
    assert!(
        is_session_worktree(&root, &session_wt),
        ".wt/<name> without .start.json is a session worktree"
    );
    assert!(
        is_session_worktree(&root, &claude_wt),
        ".claude/worktrees/<name> is a session worktree"
    );
}

#[test]
fn idle_check_errors_instead_of_guessing_when_git_dir_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let err = blyte_cli::clean::is_worktree_idle(dir.path(), 7).unwrap_err();
    assert!(err.contains("absolute-git-dir"), "got: {err}");
}
