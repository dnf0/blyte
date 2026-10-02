use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::{format_gate_drift_warning, is_ci_step_covered, missing_ci_steps};

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
fn helper_computes_missing_steps_and_formats_warning() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"[dispatch]
gate = """
make lint
make test
"""
"#;
    let missing = missing_ci_steps(r.path(), "make lint\nmake test", toml);
    assert_eq!(missing, vec!["make typecheck"]);
    let warning = format_gate_drift_warning(&missing);
    assert_eq!(
        warning.as_deref(),
        Some(
            "blyte: the gate is missing CI steps: make typecheck (run blyte init --force to refresh blyte.toml)"
        )
    );
}

#[test]
fn dispatch_warns_when_gate_is_missing_ci_step() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"[dispatch]
gate = """
make lint
make test
"""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let out = blyte(r.path(), &["dispatch", "T1", "task.md", "--sandbox", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = "blyte: the gate is missing CI steps: make typecheck (run blyte init --force to refresh blyte.toml)";
    assert!(
        stderr.contains(expected),
        "expected gate drift warning in stderr, got:\n{stderr}"
    );
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("the gate is missing CI steps"))
            .count(),
        1,
        "warning should be printed exactly once"
    );
}

#[test]
fn all_three_steps_in_gate_produces_no_warning() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"[dispatch]
gate = """
make lint
make test
make typecheck
"""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    // Direct helper check
    let missing = missing_ci_steps(r.path(), "make lint\nmake test\nmake typecheck", toml);
    assert!(missing.is_empty());
    assert_eq!(format_gate_drift_warning(&missing), None);

    // End-to-end dispatch check
    let out = blyte(r.path(), &["dispatch", "T2", "task.md", "--sandbox", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn left_out_comment_produces_no_warning() {
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

    // Direct helper check
    let missing = missing_ci_steps(r.path(), "make lint\nmake test", toml);
    assert!(missing.is_empty());
    assert_eq!(format_gate_drift_warning(&missing), None);

    // End-to-end dispatch check
    let out = blyte(r.path(), &["dispatch", "T3", "task.md", "--sandbox", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn explicit_gate_flag_bypasses_check() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"[dispatch]
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
            "T4",
            "task.md",
            "--gate",
            "make lint",
            "--sandbox",
            "off",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning with explicit --gate:\n{stderr}"
    );
}

#[test]
fn multiple_missing_ci_steps_are_all_named() {
    let ci_yml = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make lint
      - run: make test
      - run: make typecheck
      - run: make validate
"#;
    let r = repo_with_ci(ci_yml);
    let toml = r#"[dispatch]
gate = """
make lint
make test
"""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(r.path(), "make lint\nmake test", toml);
    assert_eq!(missing, vec!["make typecheck", "make validate"]);
    let warning = format_gate_drift_warning(&missing).unwrap();
    assert_eq!(
        warning,
        "blyte: the gate is missing CI steps: make typecheck, make validate (run blyte init --force to refresh blyte.toml)"
    );

    let out = blyte(r.path(), &["dispatch", "T5", "task.md", "--sandbox", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = "blyte: the gate is missing CI steps: make typecheck, make validate (run blyte init --force to refresh blyte.toml)";
    assert!(
        stderr.contains(expected),
        "expected multiple missing steps in stderr, got:\n{stderr}"
    );
}

#[test]
fn no_ci_workflows_produces_no_warning() {
    let r = repo();
    let toml = r#"[dispatch]
gate = "make test"
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(r.path(), "make test", toml);
    assert!(missing.is_empty());
    assert_eq!(format_gate_drift_warning(&missing), None);

    let out = blyte(r.path(), &["dispatch", "T6", "task.md", "--sandbox", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning when no CI workflows exist:\n{stderr}"
    );
}

#[test]
fn research_flag_bypasses_check() {
    let r = repo_with_ci(CI_YML_THREE_STEPS);
    let toml = r#"[dispatch]
gate = """
make lint
make test
"""
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "research question\n").unwrap();

    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "Q1",
            "task.md",
            "--research",
            "--sandbox",
            "off",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning with --research:\n{stderr}"
    );
}

#[test]
fn is_ci_step_covered_handles_edge_cases() {
    // Exact match in gate
    assert!(is_ci_step_covered("make test", "make test", ""));
    assert!(is_ci_step_covered(
        "make test",
        "make lint\n  make test  \nmake check",
        ""
    ));

    // Not in gate, no comments
    assert!(!is_ci_step_covered(
        "make typecheck",
        "make lint\nmake test",
        ""
    ));

    // Covered by left out comment formats
    assert!(is_ci_step_covered(
        "make typecheck",
        "",
        "# make typecheck: from ci.yml, left out: pyright is missing\n"
    ));
    assert!(is_ci_step_covered(
        "npm test",
        "",
        "# npm test: from detected, left out: jest failed to start\n"
    ));
    assert!(is_ci_step_covered(
        "make typecheck",
        "",
        "#   make typecheck: left out: env issue\n"
    ));
    assert!(is_ci_step_covered(
        "make typecheck",
        "",
        "# make typecheck: Left Out\n"
    ));

    // Non-left-out comments do NOT cover the step
    assert!(!is_ci_step_covered(
        "make typecheck",
        "",
        "# make typecheck: from ci.yml, passed\n"
    ));
    assert!(!is_ci_step_covered(
        "make typecheck",
        "",
        "# make typecheck: from ci.yml, failed on HEAD: fix it\n"
    ));
    assert!(!is_ci_step_covered(
        "make typecheck",
        "",
        "# make typecheck: not run\n"
    ));

    // Prefix collision: "make test" should not match comment for "make testing"
    assert!(!is_ci_step_covered(
        "make test",
        "",
        "# make testing: from ci.yml, left out: foo\n"
    ));
}
