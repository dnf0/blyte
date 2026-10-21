//! CI step working directory, literal env values, and reporting steps left out.

use std::path::Path;
use std::process::Command;

use blyte_cli::init::{detect_ci, detect_ci_left_out};

fn run_detect(workflow: &str) -> Vec<String> {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(&wf, workflow).unwrap();
    detect_ci(d.path())
}

fn run_detect_left_out(workflow: &str) -> Vec<(String, String, String)> {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(&wf, workflow).unwrap();
    detect_ci_left_out(d.path())
}

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

#[test]
fn workflow_default_working_directory() {
    let wf = "\
defaults:
  run:
    working-directory: svc
jobs:
  test:
    steps:
      - run: uv run --frozen pytest
";
    assert_eq!(run_detect(wf), vec!["cd svc && uv run --frozen pytest"]);
}

#[test]
fn step_working_directory_overrides_job_default() {
    let wf = "\
jobs:
  test:
    defaults:
      run:
        working-directory: a
    steps:
      - working-directory: b
        run: uv run pytest
      - run: uv run ruff check .
";
    assert_eq!(
        run_detect(wf),
        vec!["cd b && uv run pytest", "cd a && uv run ruff check ."]
    );
}

#[test]
fn matrix_working_directory() {
    let wf = "\
jobs:
  test:
    strategy:
      matrix:
        service: [x, y]
    steps:
      - working-directory: ${{ matrix.service }}
        run: make test
";
    assert_eq!(
        run_detect(wf),
        vec!["cd x && make test", "cd y && make test"]
    );
}

#[test]
fn step_env_literals_are_prefixed() {
    let wf = "\
env:
  FORCE_COLOR: \"1\"
jobs:
  test:
    strategy:
      matrix:
        service: [s1]
    steps:
      - env:
          AWS_REGION: eu-west-1
          ENVIRONMENT: staging
          LIST: \"a,b c\"
        run: make -C ${{ matrix.service }} test-coverage
";
    assert_eq!(
        run_detect(wf),
        vec!["env AWS_REGION=eu-west-1 ENVIRONMENT=staging FORCE_COLOR=1 LIST='a,b c' make -C s1 test-coverage"]
    );
}

#[test]
fn env_from_secret_leaves_step_out() {
    let wf = "\
jobs:
  test:
    steps:
      - env:
          TOKEN: ${{ secrets.T }}
        run: uv run pytest
      - run: uv run ruff check .
";
    assert_eq!(run_detect(wf), vec!["uv run ruff check ."]);
    let left_out = run_detect_left_out(wf);
    assert_eq!(left_out.len(), 1);
    assert_eq!(left_out[0].0, "uv run pytest");
    assert_eq!(left_out[0].1, "ci.yml");
    assert!(
        left_out[0]
            .2
            .contains("env TOKEN is set from a CI expression"),
        "{:?}",
        left_out[0]
    );
}

#[test]
fn unknown_matrix_key_is_reported() {
    let wf = "\
jobs:
  test:
    steps:
      - run: make -C ${{ matrix.nope }} test
";
    assert!(run_detect(wf).is_empty());
    let left_out = run_detect_left_out(wf);
    assert_eq!(left_out.len(), 1);
    assert_eq!(left_out[0].0, "make -C ${{ matrix.nope }} test");
    assert_eq!(left_out[0].1, "ci.yml");
    assert!(
        left_out[0].2.contains("unknown matrix key nope"),
        "{:?}",
        left_out[0]
    );
}

#[test]
fn non_check_steps_ignored() {
    let wf = "\
jobs:
  test:
    steps:
      - env:
          TOKEN: ${{ secrets.T }}
        run: sudo apt-get install -y gdal-bin
";
    assert!(run_detect(wf).is_empty());
    assert!(run_detect_left_out(wf).is_empty());
}

#[test]
fn init_print_reports_left_out() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    let wf = r.join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(
        &wf,
        "\
jobs:
  test:
    steps:
      - env:
          TOKEN: ${{ secrets.T }}
        run: uv run pytest
      - run: uv run ruff check .
",
    )
    .unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["init", "--print", "--no-verify"])
        .current_dir(r)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains(
            "# uv run pytest: from ci.yml, left out: env TOKEN is set from a CI expression"
        ),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("left out: env TOKEN is set from a CI expression"),
        "stderr:\n{stderr}"
    );
}
