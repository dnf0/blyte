//! `blyte dispatch --research` checks the brief and prints the template; `blyte review`
//! flags broad research runs and suggests splitting budget-exhausted runs.

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

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init", "--no-gpg-sign"]);
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
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env_remove("BLYTE_MODEL")
        .env("HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn research_dispatch_without_a_question_prints_the_template() {
    let r = repo();
    let out = blyte(r.path(), &["dispatch", "R1", "--research"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("A research question needs these sections:"),
        "{stderr}"
    );
    assert!(stderr.contains("## Start from"), "{stderr}");
    assert!(stderr.contains("## Answer format"), "{stderr}");
}

#[test]
fn research_dispatch_warns_about_missing_sections_and_goes_on() {
    let r = repo();
    std::fs::write(r.path().join("q.md"), "## Objective\nFind out X.\n").unwrap();
    let out = blyte(
        r.path(),
        &["dispatch", "R1", "q.md", "--research", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: the research question has no section for: Start from, Out of scope, Answer format"),
        "{stderr}"
    );
    // A warning, not a refusal: the dispatch went on and saved the question.
    assert!(r.path().join(".wt/R1.task.md").exists(), "{stderr}");
}

#[test]
fn research_dispatch_refuses_an_unverified_report() {
    let r = repo();
    let unv = r.path().join("prev.report.md");
    std::fs::write(
        &unv,
        "> **Not verified.** This research run ended as `budget_exhausted`.\n\n# Draft\n",
    )
    .unwrap();
    let q = format!(
        "## Objective\nFollow up on `{}`.\n\n## Start from\nf.txt\n\n## Out of scope\nnothing\n\n## Answer format\nshort\n",
        unv.display()
    );
    std::fs::write(r.path().join("q.md"), q).unwrap();
    let out = blyte(
        r.path(),
        &["dispatch", "R1", "q.md", "--research", "--sandbox", "off"],
    );
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&unv.display().to_string()), "{stderr}");
    assert!(stderr.contains("did not pass blyte's checks"), "{stderr}");
    assert!(!r.path().join(".wt/R1.task.md").exists());
    assert!(!r.path().join(".wt/R1").exists());
}

#[test]
fn no_brief_check_gets_past_both() {
    let r = repo();
    std::fs::write(r.path().join("q.md"), "## Objective\nFind out X.\n").unwrap();
    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "R1",
            "q.md",
            "--research",
            "--no-brief-check",
            "--sandbox",
            "off",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("has no section for"),
        "unexpected brief check failure:\n{stderr}"
    );
}

#[test]
fn an_implementation_dispatch_is_not_brief_checked() {
    let r = repo();
    std::fs::write(r.path().join("q.md"), "## Objective\nFind out X.\n").unwrap();
    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "R1",
            "q.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("has no section for"),
        "unexpected brief check on implementation dispatch:\n{stderr}"
    );
}

fn setup_review_worktree(r: &Path, id: &str) {
    git(
        r,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("wip/{id}"),
            &format!(".wt/{id}"),
        ],
    );
}

fn checks_section(stdout: &str) -> &str {
    let after = stdout
        .split_once("== checks\n")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    after
        .split_once("\n== ")
        .map(|(sec, _)| sec)
        .unwrap_or(after)
}

#[test]
fn review_flags_a_broad_research_run() {
    let r = repo();
    setup_review_worktree(r.path(), "R1");

    std::fs::write(
        r.path().join(".wt/R1.start.json"),
        serde_json::json!({
            "id": "R1",
            "research": true
        })
        .to_string(),
    )
    .unwrap();

    let turns: Vec<serde_json::Value> = (0..12)
        .map(|_| serde_json::json!({"input": 120000, "cached": 0, "output": 100}))
        .collect();
    std::fs::write(
        r.path().join(".wt/R1.report.json"),
        serde_json::json!({
            "outcome": "budget_exhausted",
            "turn_usage": turns
        })
        .to_string(),
    )
    .unwrap();

    let log = format!(
        "  ● read {}/crates/a/lib.rs -> 100 lines\n  ● read {}/crates/b/mod.rs -> 50 lines\n",
        r.path().display(),
        r.path().display()
    );
    std::fs::write(r.path().join(".wt/R1.log"), log).unwrap();

    let out = blyte(r.path(), &["review", "R1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let checks = checks_section(&stdout);
    assert!(
        checks.lines().any(|l| l
            .trim_start_matches("! ")
            .starts_with("broad research run: 12 of 12 turns")),
        "missing broad research run line in checks:\n{checks}"
    );
    assert!(
        checks.lines().any(|l| {
            l.trim_start_matches("! ")
                .starts_with("ran out of budget: split the question; files read by directory:")
        }),
        "missing split suggestion line in checks:\n{checks}"
    );
}

#[test]
fn review_does_not_flag_a_narrow_or_non_research_run() {
    let r = repo();

    // 1. Narrow research run (3 large turns, outcome verified)
    setup_review_worktree(r.path(), "R1");
    std::fs::write(
        r.path().join(".wt/R1.start.json"),
        serde_json::json!({
            "id": "R1",
            "research": true
        })
        .to_string(),
    )
    .unwrap();
    let narrow_turns: Vec<serde_json::Value> = (0..3)
        .map(|_| serde_json::json!({"input": 120000, "cached": 0, "output": 100}))
        .collect();
    std::fs::write(
        r.path().join(".wt/R1.report.json"),
        serde_json::json!({
            "outcome": "verified",
            "turn_usage": narrow_turns
        })
        .to_string(),
    )
    .unwrap();
    let log = format!(
        "  ● read {}/crates/a/lib.rs -> 100 lines\n  ● read {}/crates/b/mod.rs -> 50 lines\n",
        r.path().display(),
        r.path().display()
    );
    std::fs::write(r.path().join(".wt/R1.log"), &log).unwrap();

    let out1 = blyte(r.path(), &["review", "R1"]);
    assert_eq!(
        out1.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    let checks1 = checks_section(&stdout1);
    assert!(
        !checks1.contains("broad research run:"),
        "unexpected broad flag in narrow run:\n{checks1}"
    );
    assert!(
        !checks1.contains("ran out of budget: split the question"),
        "unexpected split flag in verified run:\n{checks1}"
    );

    // 2. Non-research run with 12 large turns and budget_exhausted
    setup_review_worktree(r.path(), "R2");
    std::fs::write(
        r.path().join(".wt/R2.start.json"),
        serde_json::json!({
            "id": "R2"
        })
        .to_string(),
    )
    .unwrap();
    let broad_turns: Vec<serde_json::Value> = (0..12)
        .map(|_| serde_json::json!({"input": 120000, "cached": 0, "output": 100}))
        .collect();
    std::fs::write(
        r.path().join(".wt/R2.report.json"),
        serde_json::json!({
            "outcome": "budget_exhausted",
            "turn_usage": broad_turns
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(r.path().join(".wt/R2.log"), &log).unwrap();

    let out2 = blyte(r.path(), &["review", "R2"]);
    assert_eq!(
        out2.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let checks2 = checks_section(&stdout2);
    assert!(
        !checks2.contains("broad research run:"),
        "unexpected broad flag in non-research run:\n{checks2}"
    );
    assert!(
        !checks2.contains("ran out of budget: split the question"),
        "unexpected split flag in non-research run:\n{checks2}"
    );
}
