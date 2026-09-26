//! After a successful `git commit`, the result carries the new commit and the working tree state,
//! so the model has no reason to spend turns on `git log -1` and `git status`.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             echo 0 > a.txt && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    d
}

fn bash(d: &tempfile::TempDir, command: &str) -> String {
    execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({ "command": command }),
    )
    .content
}

#[test]
fn a_successful_commit_reports_the_commit_and_a_clean_tree() {
    let d = repo();
    let out = bash(
        &d,
        "echo 1 > a.txt && git add -A && git commit -q -m 'feat: change a'",
    );
    assert!(out.starts_with("exit 0"), "{out}");
    assert!(out.contains("[after commit]"), "{out}");
    assert!(
        out.contains("feat: change a"),
        "the new commit's subject: {out}"
    );
    assert!(out.contains("a.txt"), "the files it changed: {out}");
    assert!(out.contains("working tree clean"), "{out}");
}

#[test]
fn leftover_changes_are_listed() {
    let d = repo();
    let out = bash(
        &d,
        "echo 1 > a.txt && echo x > b.txt && git add a.txt && git commit -q -m 'feat: a only'",
    );
    assert!(out.contains("[after commit]"), "{out}");
    assert!(
        out.contains("?? b.txt"),
        "the untracked file is shown: {out}"
    );
    assert!(!out.contains("working tree clean"), "{out}");
}

#[test]
fn a_failed_commit_gets_no_footer() {
    let d = repo();
    let out = bash(&d, "git commit -q -m 'nothing to commit'");
    assert!(!out.starts_with("exit 0"), "{out}");
    assert!(!out.contains("[after commit]"), "{out}");
}

#[test]
fn other_commands_get_no_footer() {
    let d = repo();
    let out = bash(&d, "git log --oneline -1");
    assert!(!out.contains("[after commit]"), "{out}");
    let out = bash(&d, "echo 'we will git commit later'");
    assert!(
        !out.contains("[after commit]"),
        "text that merely mentions a commit is not one: {out}"
    );
}
