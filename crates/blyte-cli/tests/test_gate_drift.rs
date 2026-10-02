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
fn pigeonhole_table_row_gate_covers_adapted_tool_run() {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let ci_yml = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: uv run pytest tests/
"#;
    std::fs::write(wf_dir.join("ci.yml"), ci_yml).unwrap();
    std::fs::write(
        r.path().join("pyproject.toml"),
        "[project]\nname = \"pigeonhole\"\n",
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
            "ci and pyproject",
            "--no-gpg-sign",
        ],
    );

    let toml = r#"[dispatch]
gate = "uv run --frozen --offline --extra build pytest -q -p no:cacheprovider"
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(
        r.path(),
        "uv run --frozen --offline --extra build pytest -q -p no:cacheprovider",
        toml,
    );
    assert!(
        missing.is_empty(),
        "expected no missing steps, got: {missing:?}"
    );
    assert_eq!(format_gate_drift_warning(&missing), None);

    let out = blyte(
        r.path(),
        &["dispatch", "T_pigeonhole", "task.md", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn provision_table_row_gate_pipeline_covers_ci_step() {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let ci_yml = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: pre-commit run --all-files
"#;
    std::fs::write(wf_dir.join("ci.yml"), ci_yml).unwrap();
    std::fs::write(r.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
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
            "ci and pre-commit config",
            "--no-gpg-sign",
        ],
    );

    let gate_cmd = "git diff HEAD~1 | xargs -r pre-commit run --files";
    let toml = format!(
        r#"[dispatch]
gate = "{gate_cmd}"
"#
    );
    std::fs::write(r.path().join("blyte.toml"), &toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(r.path(), gate_cmd, &toml);
    assert!(
        missing.is_empty(),
        "expected no missing steps, got: {missing:?}"
    );
    assert_eq!(format_gate_drift_warning(&missing), None);

    let out = blyte(
        r.path(),
        &["dispatch", "T_provision", "task.md", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn climate_x_services_table_row_gate_covers_adapted_tool_run() {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let ci_yml = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: pytest tests -rx --random-order
"#;
    std::fs::write(wf_dir.join("ci.yml"), ci_yml).unwrap();
    std::fs::write(
        r.path().join("pyproject.toml"),
        "[project]\nname = \"cxs\"\n",
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
            "ci and pyproject",
            "--no-gpg-sign",
        ],
    );

    let gate_cmd = "uvx --from uv==0.12.3 uv run --no-sync pytest tests/api -q";
    let toml = format!(
        r#"[dispatch]
gate = "{gate_cmd}"
"#
    );
    std::fs::write(r.path().join("blyte.toml"), &toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(r.path(), gate_cmd, &toml);
    assert!(
        missing.is_empty(),
        "expected no missing steps, got: {missing:?}"
    );
    assert_eq!(format_gate_drift_warning(&missing), None);

    let out = blyte(
        r.path(),
        &["dispatch", "T_cxs", "task.md", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn tile_service_table_row_comment_covers_ci_step() {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let ci_yml = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make test
"#;
    std::fs::write(wf_dir.join("ci.yml"), ci_yml).unwrap();
    std::fs::write(r.path().join("Makefile"), "test:\n\t@echo test\n").unwrap();
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
            "ci and makefile",
            "--no-gpg-sign",
        ],
    );

    let toml = r#"# `make test` has no recipe …, so the gate runs pytest directly
[dispatch]
gate = "pytest"
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(r.path(), "pytest", toml);
    assert!(
        missing.is_empty(),
        "expected no missing steps, got: {missing:?}"
    );
    assert_eq!(format_gate_drift_warning(&missing), None);

    let out = blyte(
        r.path(),
        &["dispatch", "T_tile", "task.md", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn geo_jobs_dashboard_table_row_missing_project_file_covers_ci_step() {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let ci_yml = r#"name: CI
on: [push]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test --workspace --all-features
"#;
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
            "ci without Cargo.toml",
            "--no-gpg-sign",
        ],
    );

    assert!(!r.path().join("Cargo.toml").exists());

    let toml = r#"[dispatch]
gate = "echo ok"
"#;
    std::fs::write(r.path().join("blyte.toml"), toml).unwrap();
    std::fs::write(r.path().join("task.md"), "dummy task\n").unwrap();

    let missing = missing_ci_steps(r.path(), "echo ok", toml);
    assert!(
        missing.is_empty(),
        "expected no missing steps, got: {missing:?}"
    );
    assert_eq!(format_gate_drift_warning(&missing), None);

    let out = blyte(
        r.path(),
        &["dispatch", "T_geo", "task.md", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("blyte: the gate is missing CI steps"),
        "unexpected gate drift warning in stderr:\n{stderr}"
    );
}

#[test]
fn climate_x_data_platform_real_case_names_missing_typecheck_and_validate() {
    let r = repo();
    let wf_dir = r.path().join(".github").join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
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
            "ci and makefile with 4 targets",
            "--no-gpg-sign",
        ],
    );

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

    let out = blyte(
        r.path(),
        &["dispatch", "T_cxdp", "task.md", "--sandbox", "off"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = "blyte: the gate is missing CI steps: make typecheck, make validate (run blyte init --force to refresh blyte.toml)";
    assert!(
        stderr.contains(expected),
        "expected missing make typecheck and make validate in stderr, got:\n{stderr}"
    );
}

#[test]
fn is_ci_step_covered_handles_key_based_edge_cases() {
    let d = tempfile::tempdir().unwrap();

    // 1. Gate matches by key despite different wrappers/flags
    assert!(is_ci_step_covered(
        d.path(),
        "uv run pytest tests/",
        "uv run --frozen --offline pytest -q",
        ""
    ));
    assert!(is_ci_step_covered(
        d.path(),
        "pre-commit run --all-files",
        "git diff | xargs -r pre-commit run --files",
        ""
    ));
    assert!(is_ci_step_covered(
        d.path(),
        "pytest tests -rx",
        "uvx --from uv==0.12.3 uv run --no-sync pytest tests/api -q",
        ""
    ));
    assert!(is_ci_step_covered(
        d.path(),
        "cargo test --workspace --all-features",
        "cargo test",
        ""
    ));
    assert!(is_ci_step_covered(d.path(), "npm test", "npm run test", ""));
    assert!(is_ci_step_covered(
        d.path(),
        "npm run lint",
        "npm run lint --fix",
        ""
    ));
    assert!(is_ci_step_covered(
        d.path(),
        "ruff check --select E,F",
        "uv run ruff check",
        ""
    ));

    // 2. Unrecognised steps are ignored (never warned about)
    assert!(is_ci_step_covered(d.path(), "echo hello", "", ""));
    assert!(is_ci_step_covered(d.path(), "mkdir -p out", "", ""));

    // 3. Comments cover the step if they contain the key text
    assert!(is_ci_step_covered(
        d.path(),
        "make test",
        "",
        "# `make test` has no recipe …, so the gate runs pytest directly\n"
    ));
    assert!(is_ci_step_covered(
        d.path(),
        "pytest tests/",
        "",
        "# pytest: left out: broken upstream\n"
    ));
    assert!(is_ci_step_covered(
        d.path(),
        "npm test",
        "",
        "# npm test: disabled for now\n"
    ));

    // 4. Prefix collision in comments: "make test" should not match comment for "make testing"
    std::fs::write(
        d.path().join("Makefile"),
        "test:\n\t@echo\ntesting:\n\t@echo\n",
    )
    .unwrap();
    assert!(!is_ci_step_covered(
        d.path(),
        "make test",
        "",
        "# make testing: from ci.yml, left out: foo\n"
    ));

    // 5. Missing project file covers the step
    let empty_dir = tempfile::tempdir().unwrap();
    assert!(is_ci_step_covered(empty_dir.path(), "cargo test", "", ""));
    assert!(is_ci_step_covered(empty_dir.path(), "go test", "", ""));
    assert!(is_ci_step_covered(empty_dir.path(), "npm test", "", ""));
    assert!(is_ci_step_covered(empty_dir.path(), "pytest", "", ""));
    assert!(is_ci_step_covered(
        empty_dir.path(),
        "pre-commit run",
        "",
        ""
    ));
    assert!(is_ci_step_covered(empty_dir.path(), "make lint", "", ""));

    // 6. When project file IS present, step is NOT covered without gate or comment
    std::fs::write(
        d.path().join("Cargo.toml"),
        "[package]\nname=\"foo\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    assert!(!is_ci_step_covered(d.path(), "cargo test", "", ""));
}
