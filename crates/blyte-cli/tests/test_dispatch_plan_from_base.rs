use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use blyte_cli::dispatch::load_task_text;

static CWD_LOCK: Mutex<()> = Mutex::new(());

struct CwdGuard {
    prev: PathBuf,
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

fn with_cwd<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _lock = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    let _guard = CwdGuard { prev };
    std::env::set_current_dir(dir).unwrap();
    f()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo_with_plan_on_branch() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(d.path().join("README.md"), "init\n").unwrap();
    git(d.path(), &["add", "."]);
    git(
        d.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
            "--no-gpg-sign",
        ],
    );

    git(d.path(), &["checkout", "-qb", "wip/p1"]);
    std::fs::create_dir_all(d.path().join("docs/plans")).unwrap();
    std::fs::write(
        d.path().join("docs/plans/p.md"),
        "# Plan\n\n### Task A1: one\nBody from branch wip/p1.\n",
    )
    .unwrap();
    git(d.path(), &["add", "docs/plans/p.md"]);
    git(
        d.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "add plan",
            "--no-gpg-sign",
        ],
    );

    git(d.path(), &["checkout", "-q", "main"]);
    assert!(!d.path().join("docs/plans/p.md").exists());
    d
}

#[test]
fn reads_plan_from_base_branch_when_absent_in_checkout() {
    let r = repo_with_plan_on_branch();
    let res = with_cwd(r.path(), || {
        load_task_text(
            None,
            Some("docs/plans/p.md"),
            Some("Task A1"),
            r.path(),
            Some("wip/p1"),
        )
    });
    let (text, recorded) = res.unwrap().unwrap();
    assert!(recorded.is_none());
    assert!(
        text.contains("Body from branch wip/p1."),
        "unexpected text: {text}"
    );
    assert!(text.contains("(on wip/p1)"), "unexpected text: {text}");
    assert!(
        text.starts_with("From the plan docs/plans/p.md (on wip/p1):\n\n"),
        "unexpected header: {text}"
    );
}

#[test]
fn missing_plan_without_base_reports_not_found_in_checkout_only() {
    let r = repo_with_plan_on_branch();
    let err = with_cwd(r.path(), || {
        load_task_text(
            None,
            Some("docs/plans/p.md"),
            Some("Task A1"),
            r.path(),
            None,
        )
    })
    .unwrap_err();
    assert!(
        err.contains("not found in the checkout"),
        "unexpected error: {err}"
    );
    assert!(!err.contains("or on"), "unexpected error: {err}");
}

#[test]
fn missing_plan_on_main_base_reports_or_on_main() {
    let r = repo_with_plan_on_branch();
    let err = with_cwd(r.path(), || {
        load_task_text(
            None,
            Some("docs/plans/p.md"),
            Some("Task A1"),
            r.path(),
            Some("main"),
        )
    })
    .unwrap_err();
    assert!(
        err.contains("not found in the checkout"),
        "unexpected error: {err}"
    );
    assert!(err.contains("or on main"), "unexpected error: {err}");
}

#[test]
fn checkout_plan_wins_over_base_branch() {
    let r = repo_with_plan_on_branch();
    std::fs::create_dir_all(r.path().join("docs/plans")).unwrap();
    std::fs::write(
        r.path().join("docs/plans/p.md"),
        "# Plan\n\n### Task A1: one\nBody from local checkout.\n",
    )
    .unwrap();

    let res = with_cwd(r.path(), || {
        load_task_text(
            None,
            Some("docs/plans/p.md"),
            Some("Task A1"),
            r.path(),
            Some("wip/p1"),
        )
    });
    let (text, recorded) = res.unwrap().unwrap();
    assert!(recorded.is_none());
    assert!(
        text.contains("Body from local checkout."),
        "unexpected text: {text}"
    );
    assert!(
        !text.contains("Body from branch wip/p1."),
        "unexpected text: {text}"
    );
    assert!(
        text.starts_with("From the plan docs/plans/p.md:\n\n"),
        "unexpected header: {text}"
    );
    assert!(!text.contains("(on wip/p1)"), "unexpected text: {text}");
}

#[test]
fn absolute_missing_plan_is_not_looked_up_on_base() {
    let r = repo_with_plan_on_branch();
    let abs_path = r.path().join("docs/plans/p.md");
    let abs_str = abs_path.to_str().unwrap();

    let err = with_cwd(r.path(), || {
        load_task_text(
            None,
            Some(abs_str),
            Some("Task A1"),
            r.path(),
            Some("wip/p1"),
        )
    })
    .unwrap_err();
    assert!(
        err.contains("not found in the checkout"),
        "unexpected error: {err}"
    );
    assert!(!err.contains("or on"), "unexpected error: {err}");
}

#[test]
fn leading_dot_slash_plan_path_reads_from_base() {
    let r = repo_with_plan_on_branch();
    let res = with_cwd(r.path(), || {
        load_task_text(
            None,
            Some("./docs/plans/p.md"),
            Some("Task A1"),
            r.path(),
            Some("wip/p1"),
        )
    });
    let (text, recorded) = res.unwrap().unwrap();
    assert!(recorded.is_none());
    assert!(
        text.contains("Body from branch wip/p1."),
        "unexpected text: {text}"
    );
    assert!(text.contains("(on wip/p1)"), "unexpected text: {text}");
}
