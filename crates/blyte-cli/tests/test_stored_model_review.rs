use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::model_review::{Concern, ModelReviewResult};
use blyte_cli::queue_actions::read_stored_model_review;
use blyte_cli::replay;

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

fn init_repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/a.rs"), "fn f() {}\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init", "--no-gpg-sign"]);
    d
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
    std::fs::write(r.join("task.md"), "Files: `src/a.rs`\n").unwrap();
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
        serde_json::json!({ "outcome": "verified" }).to_string(),
    )
    .unwrap();
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
        .env("XDG_CONFIG_HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn missing_file_returns_ok_none() {
    let d = tempfile::tempdir().unwrap();
    let res = read_stored_model_review(d.path(), "R1").unwrap();
    assert!(res.is_none());
}

#[test]
fn missing_model_review_key_returns_ok_none() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join("R1.checks.json"), r#"{"ts":1,"flags":[]}"#).unwrap();

    let res = read_stored_model_review(d.path(), "R1").unwrap();
    assert!(res.is_none());
}

#[test]
fn null_model_review_returns_ok_none() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(
        wt.join("R1.checks.json"),
        r#"{"ts":1,"flags":[],"model_review":null}"#,
    )
    .unwrap();

    let res = read_stored_model_review(d.path(), "R1").unwrap();
    assert!(res.is_none());
}

#[test]
fn stored_model_review_returns_ok_some() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let stored = ModelReviewResult::ok(
        "test-model".to_string(),
        Some(0.01),
        vec![Concern {
            file: "src/a.rs".to_string(),
            line: Some(3),
            severity: "high".to_string(),
            text: "missing check".to_string(),
        }],
    );
    let body = serde_json::json!({
        "ts": 1,
        "flags": [],
        "model_review": stored,
    });
    std::fs::write(wt.join("R1.checks.json"), body.to_string()).unwrap();

    let got = read_stored_model_review(d.path(), "R1")
        .unwrap()
        .expect("expected stored review");
    assert_eq!(got.status, "ok");
    assert_eq!(got.model.as_deref(), Some("test-model"));
    assert_eq!(got.concerns.len(), 1);
    assert_eq!(got.concerns[0].file, "src/a.rs");
    assert_eq!(got.concerns[0].line, Some(3));
    assert_eq!(got.concerns[0].text, "missing check");
}

#[test]
fn invalid_json_returns_error_naming_file() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let checks_path = wt.join("R1.checks.json");
    std::fs::write(&checks_path, "{not valid json").unwrap();

    let err = read_stored_model_review(d.path(), "R1").unwrap_err();
    assert!(
        err.contains(&checks_path.display().to_string()),
        "error should name file, got: {err}"
    );
    assert!(err.contains("cannot parse"), "got: {err}");
}

#[test]
fn non_object_json_returns_error_naming_file() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let checks_path = wt.join("R1.checks.json");
    std::fs::write(&checks_path, "[]").unwrap();

    let err = read_stored_model_review(d.path(), "R1").unwrap_err();
    assert!(
        err.contains(&checks_path.display().to_string()),
        "error should name file, got: {err}"
    );
    assert!(err.contains("JSON object"), "got: {err}");
}

#[test]
fn unreadable_checks_path_returns_error_naming_file() {
    let d = tempfile::tempdir().unwrap();
    let checks_path = d.path().join(".wt/R1.checks.json");
    std::fs::create_dir_all(&checks_path).unwrap();

    let err = read_stored_model_review(d.path(), "R1").unwrap_err();
    assert!(
        err.contains(&checks_path.display().to_string()),
        "error should name file, got: {err}"
    );
    assert!(err.contains("cannot read"), "got: {err}");
}

#[test]
fn wrong_shape_model_review_returns_error_naming_file() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let checks_path = wt.join("R1.checks.json");
    std::fs::write(
        &checks_path,
        r#"{"ts":1,"flags":[],"model_review":{"concerns":"not-an-array"}}"#,
    )
    .unwrap();

    let err = read_stored_model_review(d.path(), "R1").unwrap_err();
    assert!(
        err.contains(&checks_path.display().to_string()),
        "error should name file, got: {err}"
    );
    assert!(err.contains("invalid model_review"), "got: {err}");
}

#[test]
fn blyte_review_exits_nonzero_with_file_named_on_invalid_json() {
    let r = init_repo();
    setup_run(r.path(), "R1");
    let checks_path = r.path().join(".wt/R1.checks.json");
    let broken = "{broken json";
    std::fs::write(&checks_path, broken).unwrap();

    let out = blyte(r.path(), &["review", "R1"]);
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("R1.checks.json"),
        "stderr should name file, got: {stderr}"
    );
    assert_eq!(std::fs::read_to_string(&checks_path).unwrap(), broken);
}

#[test]
fn blyte_review_exits_nonzero_with_file_named_on_wrong_shape_model_review() {
    let r = init_repo();
    setup_run(r.path(), "R2");
    let checks_path = r.path().join(".wt/R2.checks.json");
    let broken = r#"{"ts":1,"flags":[],"model_review":"wrong-shape"}"#;
    std::fs::write(&checks_path, broken).unwrap();

    let out = blyte(r.path(), &["review", "R2"]);
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("R2.checks.json"),
        "stderr should name file, got: {stderr}"
    );
    assert!(
        stderr.contains("invalid model_review"),
        "stderr should name cause, got: {stderr}"
    );
    assert_eq!(std::fs::read_to_string(&checks_path).unwrap(), broken);
}

#[test]
fn replay_treats_unreadable_model_review_as_waiting_and_shows_error() {
    let d = tempfile::tempdir().unwrap();
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wt = d.path().join("repos/repo-a/.wt");
    std::fs::create_dir_all(&wt).unwrap();

    std::fs::write(
        wt.join("M1.start.json"),
        format!(r#"{{"id":"M1","pid":999999,"started_at":{}}}"#, t - 200),
    )
    .unwrap();
    std::fs::write(wt.join("M1.log"), "[Turn 1] \n").unwrap();
    std::fs::write(wt.join("M1.json"), r#"{"outcome":"verified","seconds":30}"#).unwrap();
    let checks_path = wt.join("M1.checks.json");
    std::fs::write(
        &checks_path,
        r#"{"ts":1,"flags":[],"model_review":{"status":123}}"#,
    )
    .unwrap();

    let src = Sources {
        roots: vec![d.path().join("repos")],
        runs_dirs: vec![],
        hours: 0,
    };
    let result = replay::collect(&src, t, 7);
    assert!(result.skipped.is_empty(), "{:?}", result.skipped);
    assert_eq!(result.runs.len(), 1);
    let run = &result.runs[0];
    assert_eq!(run.id, "M1");
    assert_eq!(run.card, "exception");
    assert!(run.verdict.is_none());
    assert!(
        run.flags
            .iter()
            .any(|f| f.contains("M1.checks.json") && f.contains("invalid model_review")),
        "expected flag naming file and cause, got: {:?}",
        run.flags
    );
}
