//! Tests for `blyte dispatch <ID>` worktree reuse, running ID checks, and summary clearing.

use std::path::Path;
use std::process::Command;

fn blyte_bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
    }
    dir
}

fn run_dispatch(cwd: &Path, id: &str, task: &str) -> std::process::Output {
    let home = cwd.join("_home");
    std::fs::create_dir_all(&home).unwrap();
    let task_file = cwd.join("task.md");
    std::fs::write(&task_file, task).unwrap();
    Command::new(blyte_bin())
        .args([
            "dispatch",
            id,
            task_file.to_str().unwrap(),
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .current_dir(cwd)
        .env("HOME", &home)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .output()
        .unwrap()
}

fn git_rev_parse(cwd: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn dispatch_refuses_to_reuse_worktree_from_older_base() {
    let r = init_repo();
    let root = r.path();

    let out1 = run_dispatch(root, "T1", "first task");
    assert_eq!(out1.status.code(), Some(1));

    let wt_dir = root.join(".wt").join("T1");
    assert!(wt_dir.is_dir());
    let wt_head_before = git_rev_parse(&wt_dir, "HEAD");

    std::fs::write(root.join("new.txt"), "new content\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "new.txt"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "advance main"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    let new_main_head = git_rev_parse(root, "HEAD");

    let out2 = run_dispatch(root, "T1", "second task");
    assert_eq!(out2.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out2.stderr);
    let expected = format!(
        "error: .wt/T1 is from an earlier run based on {}, which does not contain {} (HEAD). Run `blyte clean`, remove it with `git worktree remove .wt/T1` and `git branch -D wip/T1` if it is not needed, or dispatch with a new ID.",
        &wt_head_before[..7],
        &new_main_head[..7],
    );
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\ngot:\n{stderr}"
    );
    assert!(wt_dir.is_dir(), "worktree should still exist");
    assert_eq!(
        git_rev_parse(&wt_dir, "HEAD"),
        wt_head_before,
        "worktree HEAD should be unchanged"
    );
}

#[test]
fn dispatch_reuses_worktree_when_base_has_not_moved() {
    let r = init_repo();
    let root = r.path();

    let out1 = run_dispatch(root, "T1", "first task");
    assert_eq!(out1.status.code(), Some(1));

    let out2 = run_dispatch(root, "T1", "retry task");
    assert_eq!(out2.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(root.join(".wt").join("T1.task.md")).unwrap(),
        "retry task"
    );
}

#[test]
fn dispatch_refuses_when_run_with_same_id_is_still_running() {
    let r = init_repo();
    let root = r.path();
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let start_json = serde_json::json!({
        "id": "T1",
        "pid": pid,
        "started_at": now,
    });
    std::fs::write(
        wt_dir.join("T1.start.json"),
        serde_json::to_string_pretty(&start_json).unwrap(),
    )
    .unwrap();

    let out = run_dispatch(root, "T1", "do task");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = format!(
        "error: run T1 is still running (pid {pid}). Wait for it to finish, or dispatch with a new ID."
    );
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\ngot:\n{stderr}"
    );
}

#[test]
fn dispatch_clears_stale_summary_and_keeps_verdict() {
    let r = init_repo();
    let root = r.path();
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    std::fs::write(wt_dir.join("T1.json"), r#"{"outcome":"verified"}"#).unwrap();
    std::fs::write(
        wt_dir.join("T1.verdict.json"),
        r#"{"verdict":"needs-changes"}"#,
    )
    .unwrap();

    let out = run_dispatch(root, "T1", "do task");
    assert_eq!(out.status.code(), Some(1));

    let summary_str = std::fs::read_to_string(wt_dir.join("T1.json")).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&summary_str).unwrap();
    assert_ne!(summary["outcome"], "verified");
    assert_eq!(summary["outcome"], "error");

    assert!(
        wt_dir.join("T1.verdict.json").is_file(),
        "pre-existing .wt/T1.verdict.json must be preserved"
    );
}

#[test]
fn dispatch_fails_on_unparsable_start_file() {
    let r = init_repo();
    let root = r.path();
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    let start_file = wt_dir.join("T1.start.json");
    std::fs::write(&start_file, "not json").unwrap();

    let out = run_dispatch(root, "T1", "do task");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("T1.start.json"),
        "stderr must name the start file, got: {stderr}"
    );
}
