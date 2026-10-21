use std::path::Path;
use std::process::Command;

use blyte_cli::gate_drift::left_out_steps;

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

fn repo_with_ci(ci_yml: &str) -> tempfile::TempDir {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    std::fs::write(wf_dir.join("ci.yml"), ci_yml).unwrap();
    std::fs::write(
        r.path().join("Makefile"),
        "lint:\n\t@echo lint\ntest:\n\t@echo test\ntypecheck:\n\t@echo typecheck\nvalidate:\n\t@echo validate\n",
    )
    .unwrap();
    git(r.path(), &["add", "."]);
    git(
        r.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "add ci",
            "--no-gpg-sign",
        ],
    );
    r
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

const CI_YML_THREE_STEPS: &str = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make lint
      - run: make test
      - run: make typecheck
"#;

#[test]
fn left_out_steps_extracts_three_forms_and_ignores_unrelated_comments() {
    let toml = r#"# blyte.toml: how this repository's work is checked, for `blyte dispatch`.
# make test: from ci.yml, passed
# make typecheck: from ci.yml, left out: pyright requires node
# make lint: from ci.yml, failed on HEAD: fix it before dispatching
# unrelated note about make validate
[dispatch]
# setup = "uv sync"   # left out: failed in a fresh worktree
gate = "make test"
"#;
    let steps = left_out_steps(toml);
    assert_eq!(
        steps,
        vec![
            (
                "make typecheck".to_string(),
                "pyright requires node".to_string()
            ),
            ("make lint".to_string(), "failed on HEAD".to_string()),
            (
                "setup: uv sync".to_string(),
                "failed in a fresh worktree".to_string()
            ),
        ]
    );
}

#[test]
fn dispatch_with_partial_gate_reports_left_out_steps_and_records_them() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"# make typecheck: from ci.yml, left out: pyright requires node
[dispatch]
gate = """
make lint
make test
"""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let out = blyte(r.path(), &["dispatch", "P1", "task.md", "--sandbox", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the gate is partial; left out: make typecheck (pyright requires node)"),
        "expected partial gate notice in stderr, got:\n{stderr}"
    );

    let start_path = r.path().join(".wt").join("P1.start.json");
    let start_content = std::fs::read_to_string(&start_path).expect("P1.start.json must exist");
    let start_val: serde_json::Value = serde_json::from_str(&start_content).unwrap();
    assert_eq!(
        start_val["gate_left_out"],
        serde_json::json!([{
            "command": "make typecheck",
            "reason": "pyright requires node"
        }])
    );

    let report_path = r.path().join(".wt").join("P1.report.json");
    let report_content = std::fs::read_to_string(&report_path).expect("P1.report.json must exist");
    let report_val: serde_json::Value = serde_json::from_str(&report_content).unwrap();
    assert_eq!(
        report_val["gate_left_out"],
        serde_json::json!(["make typecheck (pyright requires node)"])
    );

    let review_out = blyte(r.path(), &["review", "P1"]);
    let review_stdout = String::from_utf8_lossy(&review_out.stdout);
    assert!(
        review_stdout.contains("! gate left out: make typecheck (pyright requires node)"),
        "expected gate left out flag in review stdout, got:\n{review_stdout}"
    );
}

#[test]
fn dispatch_with_empty_gate_in_blyte_toml_exits_2_and_lists_left_out_steps() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"# make typecheck: from ci.yml, left out: pyright requires node
[dispatch]
gate = ""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let out = blyte(r.path(), &["dispatch", "P2", "task.md", "--sandbox", "off"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the gate in blyte.toml is empty"),
        "expected empty gate error in stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("  make typecheck: pyright requires node"),
        "expected left-out step line in stderr, got:\n{stderr}"
    );
}

#[test]
fn dispatch_with_explicit_gate_flag_does_not_print_partial_gate_line() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"# make typecheck: from ci.yml, left out: pyright requires node
[dispatch]
gate = """
make lint
make test
"""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "P3",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("the gate is partial"),
        "unexpected partial-gate line with --gate:\n{stderr}"
    );
}
