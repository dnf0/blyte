//! `blyte dispatch` records `base_sha` in `.wt/<id>.start.json`, and `blyte review` diffs against it
//! rather than the main checkout's current `HEAD` (2026-10-23).

use std::path::Path;
use std::process::Command;

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

fn rev_parse(dir: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse {rev}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
fn review_uses_recorded_base_sha_when_another_branch_is_checked_out() {
    let r = repo();
    let init_sha = rev_parse(r.path(), "HEAD");

    git(r.path(), &["checkout", "-qb", "feat/a"]);
    std::fs::write(r.path().join("base.txt"), "feat base\n").unwrap();
    git(r.path(), &["add", "."]);
    git(
        r.path(),
        &["commit", "-qm", "other: unrelated", "--no-gpg-sign"],
    );
    let feat_a_sha = rev_parse(r.path(), "HEAD");

    std::fs::write(
        r.path().join("task.md"),
        "Files: `f.txt`\nCommit: `feat: a1`\n",
    )
    .unwrap();

    // Dispatch A1 from feat/a so .wt/A1.start.json records base_sha = tip of feat/a.
    let _ = blyte(
        r.path(),
        &[
            "dispatch",
            "A1",
            "task.md",
            "--base",
            "feat/a",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.path().join(".wt/A1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["base_sha"], feat_a_sha);

    let wt = r.path().join(".wt/A1");
    std::fs::write(wt.join("f.txt"), "a1\n").unwrap();
    git(&wt, &["add", "f.txt"]);
    git(&wt, &["commit", "-qm", "feat: a1", "--no-gpg-sign"]);

    // Switch the main checkout to branch `other` with its own commit `other: unrelated`.
    git(r.path(), &["checkout", "-qb", "other", &init_sha]);
    std::fs::write(r.path().join("other.txt"), "other\n").unwrap();
    git(r.path(), &["add", "other.txt"]);
    git(
        r.path(),
        &["commit", "-qm", "other: unrelated", "--no-gpg-sign"],
    );

    let out = blyte(r.path(), &["review", "A1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let commits_section = text
        .split("== commits\n")
        .nth(1)
        .unwrap()
        .split("\n== ")
        .next()
        .unwrap();
    assert!(commits_section.contains("feat: a1"), "{text}");
    assert!(!text.contains("other: unrelated"), "{text}");
    assert!(
        !text.contains("commit message not in the task: other: unrelated"),
        "{text}"
    );
}

#[test]
fn old_run_without_base_sha_still_reviews_against_merge_base() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/OLD", ".wt/OLD"],
    );
    std::fs::write(
        r.path().join("task.md"),
        "Files: `f.txt`\nCommit: `feat: old`\n",
    )
    .unwrap();
    std::fs::write(
        r.path().join(".wt/OLD.start.json"),
        serde_json::json!({
            "id": "OLD",
            "task_file": "task.md"
        })
        .to_string(),
    )
    .unwrap();

    let wt = r.path().join(".wt/OLD");
    std::fs::write(wt.join("f.txt"), "old\n").unwrap();
    git(&wt, &["add", "f.txt"]);
    git(&wt, &["commit", "-qm", "feat: old", "--no-gpg-sign"]);

    let out = blyte(r.path(), &["review", "OLD"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("feat: old"), "{text}");
}

#[test]
fn missing_base_sha_commit_exits_2() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/M1", ".wt/M1"],
    );
    let missing_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    std::fs::write(
        r.path().join(".wt/M1.start.json"),
        serde_json::json!({
            "id": "M1",
            "base_sha": missing_sha
        })
        .to_string(),
    )
    .unwrap();

    let out = blyte(r.path(), &["review", "M1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "error: run M1 started from {missing_sha}, which is not in this repository"
        )),
        "{stderr}"
    );
}
