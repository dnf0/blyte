//! `blyte review` records the reviewer's verdict beside the run, and warns about commands that
//! reached outside the worktree (E5, 2026-09-27: a run deleted uv cache entries) (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::host_commands;

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
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap()
}

#[test]
fn host_commands_flags_changes_outside_the_worktree_only() {
    let log = "\
[Turn 1] \n● bash rm -rf build/ -> exit 0
● bash cat /Users/x/.zshrc > /dev/null -> exit 0
● bash cargo test 2>&1 | tail -5 -> exit 0
● bash rm -rf ~/.cache/uv/wheels-v5/pypi/ruff -> exit 0
● bash uv cache clean ruff -> exit 0
\x1b[31m● bash echo 127.0.0.1 x >> /etc/hosts -> exit 1\x1b[0m
● bash git push origin main -> exit 1
● bash uv cache clean ruff -> exit 0
";
    assert_eq!(
        host_commands(log),
        vec![
            "rm -rf ~/.cache/uv/wheels-v5/pypi/ruff",
            "uv cache clean ruff",
            "echo 127.0.0.1 x >> /etc/hosts",
            "git push origin main",
        ]
    );
    assert!(host_commands("● bash ls -> exit 0\n").is_empty());
}

#[test]
fn a_verdict_is_recorded_without_the_worktree() {
    let r = repo();
    let out = blyte(
        r.path(),
        &[
            "review",
            "T1",
            "--verdict",
            "fixed",
            "--notes",
            "tightened a test",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("recorded verdict fixed for T1"));
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/T1.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["verdict"], "fixed");
    assert_eq!(v["notes"], "tightened a test");
    assert!(v["ts"].as_u64().unwrap() > 1_700_000_000);

    assert_eq!(
        blyte(r.path(), &["review", "T1", "--verdict", "great"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        blyte(r.path(), &["review", "T1", "--notes", "x"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn review_lists_host_commands_from_the_log() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/T2", ".wt/T2"],
    );
    std::fs::write(
        r.path().join(".wt/T2.log"),
        "[Turn 1] \n● bash rm -rf ~/.cache/uv/x -> exit 0\n● bash ls -> exit 0\n",
    )
    .unwrap();
    let out = blyte(r.path(), &["review", "T2"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("== host commands"), "{text}");
    assert!(text.contains("rm -rf ~/.cache/uv/x"), "{text}");

    std::fs::write(
        r.path().join(".wt/T2.log"),
        "[Turn 1] \n● bash ls -> exit 0\n",
    )
    .unwrap();
    let text = String::from_utf8_lossy(&blyte(r.path(), &["review", "T2"]).stdout).to_string();
    assert!(!text.contains("== host commands"), "{text}");
}

#[test]
fn model_review_conflicts_with_verdict() {
    let r = repo();
    let out = blyte(
        r.path(),
        &["review", "T1", "--model-review", "--verdict", "accepted"],
    );
    assert_ne!(
        out.status.code(),
        Some(0),
        "expected clap to reject conflicting --model-review and --verdict flags"
    );
}

#[test]
fn model_review_skipped_when_task_file_not_found() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/T3", ".wt/T3"],
    );
    let out = blyte(r.path(), &["review", "T3", "--model-review"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("== model review"), "{text}");
    assert!(text.contains("skipped: task file not found"), "{text}");
}

#[test]
fn model_review_skipped_when_no_api_key() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/T4", ".wt/T4"],
    );
    std::fs::write(r.path().join("task.md"), "# Task\nDo something\n").unwrap();
    std::fs::write(
        r.path().join(".wt/T4.start.json"),
        serde_json::json!({
            "task_file": "task.md"
        })
        .to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "T4", "--model-review"])
        .current_dir(r.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env("HOME", r.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("== model review"), "{text}");
    assert!(text.contains("skipped: no API key (set "), "{text}");
}
