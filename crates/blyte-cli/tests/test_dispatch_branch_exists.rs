use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::branch_exists_message;

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

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    std::fs::write(d.path().join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env("HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn dispatch_explains_existing_unmerged_wip_branch() {
    let r = repo();
    git(r.path(), &["checkout", "-qb", "wip/A1"]);
    std::fs::write(r.path().join("f.txt"), "b\n").unwrap();
    git(r.path(), &["commit", "-qam", "work", "--no-gpg-sign"]);
    git(r.path(), &["checkout", "-q", "main"]);

    std::fs::write(r.path().join("task.md"), "Do work.\n").unwrap();
    let out = blyte(r.path(), &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "error: branch wip/A1 already exists from an earlier run and is not merged. Review it with `blyte review A1`, delete it (`git branch -D wip/A1`) if it is not needed, or dispatch with a new ID."
        ),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        branch_exists_message(r.path(), "wip/A1", "A1"),
        "error: branch wip/A1 already exists from an earlier run and is not merged. Review it with `blyte review A1`, delete it (`git branch -D wip/A1`) if it is not needed, or dispatch with a new ID."
    );
}

#[test]
fn dispatch_explains_existing_merged_wip_branch() {
    let r = repo();
    git(r.path(), &["branch", "wip/A1"]);

    std::fs::write(r.path().join("task.md"), "Do work.\n").unwrap();
    let out = blyte(r.path(), &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "error: branch wip/A1 already exists from an earlier run, and its work is already merged. Run `blyte clean` to remove it, or dispatch with a new ID."
        ),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        branch_exists_message(r.path(), "wip/A1", "A1"),
        "error: branch wip/A1 already exists from an earlier run, and its work is already merged. Run `blyte clean` to remove it, or dispatch with a new ID."
    );
}
