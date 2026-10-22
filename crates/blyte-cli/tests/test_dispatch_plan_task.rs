//! Tests for extracting dispatch tasks from a plan when no task file is given.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
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
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_STATE_DIR", dir.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn no_task_file_and_no_plan_flags_fails_fast_with_no_worktree() {
    let r = repo();
    let out = blyte(
        r.path(),
        &["dispatch", "T_none", "--gate", "true", "--sandbox", "off"],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error: give a task file, or --plan with --plan-task"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !r.path().join(".wt/T_none").exists(),
        "worktree must not be created on error"
    );
    assert!(
        !r.path().join(".wt/T_none.task.md").exists(),
        "task file copy must not be created on error"
    );
}

#[test]
fn missing_heading_in_plan_fails_fast_with_no_worktree() {
    let r = repo();
    std::fs::write(
        r.path().join("plan.md"),
        "# Plan\n\n### Task A: first\nBody A\n",
    )
    .unwrap();

    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T_miss",
            "--plan",
            "plan.md",
            "--plan-task",
            "Task Z",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no heading \"Task Z\""),
        "stderr missing error text: {stderr}"
    );
    assert!(
        stderr.contains("error: no heading \"Task Z\" in plan.md"),
        "full error expected in: {stderr}"
    );
    assert!(
        !r.path().join(".wt/T_miss").exists(),
        "worktree must not be created on error"
    );
}

#[test]
fn unreadable_plan_fails_fast_with_no_worktree() {
    let r = repo();
    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T_bad",
            "--plan",
            "nonexistent.md",
            "--plan-task",
            "Task 1",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error reading plan nonexistent.md:"),
        "unexpected stderr: {stderr}"
    );
    assert!(!r.path().join(".wt/T_bad").exists());
}

#[test]
fn only_one_of_plan_flags_given_without_task_file_errors() {
    let r = repo();
    let out1 = blyte(
        r.path(),
        &[
            "dispatch",
            "T_only_plan",
            "--plan",
            "plan.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(out1.status.code(), Some(2));
    let stderr1 = String::from_utf8_lossy(&out1.stderr);
    assert!(stderr1.contains("error: give a task file, or --plan with --plan-task"));
    assert!(!r.path().join(".wt/T_only_plan").exists());

    let out2 = blyte(
        r.path(),
        &[
            "dispatch",
            "T_only_task",
            "--plan-task",
            "Task 1",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(out2.status.code(), Some(2));
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(stderr2.contains("error: give a task file, or --plan with --plan-task"));
    assert!(!r.path().join(".wt/T_only_task").exists());
}

#[test]
fn plan_task_is_extracted_and_recorded_when_no_task_file_given() {
    let r = repo();
    std::fs::write(
        r.path().join("plan.md"),
        "# Implementation Plan\n\n### Task B2: the parser\nParse tokens into AST.\n\n### Task B3: code gen\nGenerate code.\n",
    )
    .unwrap();

    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T_plan",
            "--plan",
            "plan.md",
            "--plan-task",
            "Task B2",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );

    // Child fails with exit 1 because no API key is provided, but dispatch runs through setup
    assert_eq!(out.status.code(), Some(1));
    assert!(r.path().join(".wt/T_plan").exists());

    let task_copy = r.path().join(".wt/T_plan.task.md");
    assert!(
        task_copy.exists(),
        "task copy must be written to .wt/<id>.task.md"
    );
    let content = std::fs::read_to_string(&task_copy).unwrap();
    assert_eq!(
        content,
        "From the plan plan.md:\n\n### Task B2: the parser\nParse tokens into AST.\n\n"
    );

    let start_file = r.path().join(".wt/T_plan.start.json");
    assert!(start_file.exists());
    let start_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(start_file).unwrap()).unwrap();
    assert_eq!(
        start_json["task_file"],
        task_copy.canonicalize().unwrap().display().to_string(),
        "task_file in start.json must point to .wt/<id>.task.md when no task file was given"
    );
    assert_eq!(start_json["plan"], "plan.md");
    assert_eq!(start_json["plan_task"], "Task B2");
}

#[test]
fn dispatch_with_task_file_writes_task_copy_and_preserves_task_file_path() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "Build the feature directly.\n").unwrap();

    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T_tf",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );

    assert_eq!(out.status.code(), Some(1));
    assert!(r.path().join(".wt/T_tf").exists());

    let task_copy = r.path().join(".wt/T_tf.task.md");
    assert!(
        task_copy.exists(),
        ".wt/<id>.task.md must always be written"
    );
    let content = std::fs::read_to_string(&task_copy).unwrap();
    assert_eq!(content, "Build the feature directly.\n");

    let start_file = r.path().join(".wt/T_tf.start.json");
    let start_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(start_file).unwrap()).unwrap();
    assert_eq!(
        start_json["task_file"], "task.md",
        "task_file in start.json must keep given path when provided"
    );
}
