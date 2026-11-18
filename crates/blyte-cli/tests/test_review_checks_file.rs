//! `blyte review <id>` saves check flags and fallback findings to `.wt/<id>.checks.json`.

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
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/a.rs"), "fn f(x: Option<i32>) {}\n").unwrap();
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

fn setup_run(r: &Path, id: &str) {
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
    std::fs::write(r.join("task.md"), "Files: `src/a.rs`\nCommit: `feat: a`\n").unwrap();
    std::fs::write(
        r.join(format!(".wt/{id}.start.json")),
        serde_json::json!({
            "id": id,
            "task_file": "task.md"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        r.join(format!(".wt/{id}.report.json")),
        serde_json::json!({
            "outcome": "verified"
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn review_writes_the_checks_file() {
    let r = repo();
    setup_run(r.path(), "R1");

    let wt = r.path().join(".wt/R1");
    std::fs::write(
        wt.join("src/a.rs"),
        "fn f(x: Option<i32>) {\n    let v = x.unwrap_or(0);\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: a", "--no-gpg-sign"]);

    let out = blyte(r.path(), &["review", "R1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let checks_path = r.path().join(".wt/R1.checks.json");
    assert!(checks_path.exists());
    let val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&checks_path).unwrap()).unwrap();

    assert!(val["ts"].as_u64().unwrap() > 0);
    let flags = val["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 1);
    assert!(
        flags[0]
            .as_str()
            .unwrap()
            .starts_with("possible fallbacks added (1)"),
        "{flags:?}"
    );

    let fallbacks = val["fallbacks"].as_array().unwrap();
    assert_eq!(fallbacks.len(), 1);
    assert_eq!(fallbacks[0]["file"].as_str().unwrap(), "src/a.rs");
    assert!(fallbacks[0]["line"].as_u64().unwrap() > 0);
    assert!(!fallbacks[0]["pattern"].as_str().unwrap().is_empty());
}

#[test]
fn all_fallbacks_are_saved_not_three() {
    let r = repo();
    setup_run(r.path(), "R2");

    let wt = r.path().join(".wt/R2");
    std::fs::write(
        wt.join("src/a.rs"),
        "fn f(x: Option<i32>) {\n    let v1 = x.unwrap_or(0);\n    let v2 = x.unwrap_or(0);\n    let v3 = x.unwrap_or(0);\n    let v4 = x.unwrap_or(0);\n    let v5 = x.unwrap_or(0);\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: a", "--no-gpg-sign"]);

    let out = blyte(r.path(), &["review", "R2"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("and 2 more"), "{stdout}");

    let val: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/R2.checks.json")).unwrap(),
    )
    .unwrap();
    let flags = val["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 1);
    assert!(flags[0].as_str().unwrap().contains("and 2 more"));
    let fallbacks = val["fallbacks"].as_array().unwrap();
    assert_eq!(fallbacks.len(), 5);
}

#[test]
fn a_clean_review_writes_empty_arrays() {
    let r = repo();
    setup_run(r.path(), "R3");

    let wt = r.path().join(".wt/R3");
    std::fs::write(
        wt.join("src/a.rs"),
        "fn f(x: Option<i32>) {\n    let _ = x;\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: a", "--no-gpg-sign"]);

    let out = blyte(r.path(), &["review", "R3"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("nothing flagged"), "{stdout}");

    let val: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/R3.checks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(val["flags"].as_array().unwrap().len(), 0);
    assert_eq!(val["fallbacks"].as_array().unwrap().len(), 0);
}

#[test]
fn recording_a_verdict_leaves_the_checks_file() {
    let r = repo();
    setup_run(r.path(), "R4");

    let wt = r.path().join(".wt/R4");
    std::fs::write(
        wt.join("src/a.rs"),
        "fn f(x: Option<i32>) {\n    let v = x.unwrap_or(0);\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: a", "--no-gpg-sign"]);

    let out = blyte(r.path(), &["review", "R4"]);
    assert_eq!(out.status.code(), Some(0));

    let checks_path = r.path().join(".wt/R4.checks.json");
    let before = std::fs::read(&checks_path).unwrap();

    let v_out = blyte(r.path(), &["review", "R4", "--verdict", "accepted"]);
    assert_eq!(v_out.status.code(), Some(0));

    let after = std::fs::read(&checks_path).unwrap();
    assert_eq!(before, after);
}

#[test]
fn a_second_review_overwrites() {
    let r = repo();
    setup_run(r.path(), "R5");

    let wt = r.path().join(".wt/R5");
    std::fs::write(
        wt.join("src/a.rs"),
        "fn f(x: Option<i32>) {\n    let _ = x;\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: a", "--no-gpg-sign"]);

    let out1 = blyte(r.path(), &["review", "R5"]);
    assert_eq!(out1.status.code(), Some(0));

    let checks_path = r.path().join(".wt/R5.checks.json");
    let val1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&checks_path).unwrap()).unwrap();
    assert_eq!(val1["flags"].as_array().unwrap().len(), 0);
    assert_eq!(val1["fallbacks"].as_array().unwrap().len(), 0);

    std::fs::write(
        wt.join("src/a.rs"),
        "fn f(x: Option<i32>) {\n    let v = x.unwrap_or(0);\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: a", "--no-gpg-sign"]);

    let out2 = blyte(r.path(), &["review", "R5"]);
    assert_eq!(out2.status.code(), Some(0));

    let val2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&checks_path).unwrap()).unwrap();
    let flags = val2["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 1);
    assert!(flags[0]
        .as_str()
        .unwrap()
        .starts_with("possible fallbacks added (1)"));
    assert_eq!(val2["fallbacks"].as_array().unwrap().len(), 1);
}
