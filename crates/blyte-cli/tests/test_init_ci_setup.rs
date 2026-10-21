//! Per-directory `uv sync` steps from CI workflows become part of `setup`.

use std::path::Path;
use std::process::Command;

use blyte_cli::init::{detect_ci, detect_ci_left_out, detect_ci_setup};

fn write_workflow(workflow: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(&wf, workflow).unwrap();
    d
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
fn per_directory_uv_sync_is_setup() {
    let wf = "\
defaults:
  run:
    working-directory: worker-service
jobs:
  test:
    steps:
      - run: uv sync --frozen --all-groups
      - run: uv run --frozen --no-sync pytest
";
    let d = write_workflow(wf);
    assert_eq!(
        detect_ci_setup(d.path()),
        vec!["(cd worker-service && uv sync --frozen --all-groups)"]
    );
    assert_eq!(
        detect_ci(d.path()),
        vec!["cd worker-service && uv run --frozen --no-sync pytest"]
    );
}

#[test]
fn root_uv_sync_is_not_ci_setup() {
    let wf = "\
jobs:
  test:
    steps:
      - run: uv sync
";
    let d = write_workflow(wf);
    assert!(detect_ci_setup(d.path()).is_empty());
}

#[test]
fn matrix_directories() {
    let wf = "\
jobs:
  test:
    strategy:
      matrix:
        svc: [a, b]
    steps:
      - working-directory: ${{ matrix.svc }}
        run: uv sync
";
    let d = write_workflow(wf);
    assert_eq!(
        detect_ci_setup(d.path()),
        vec!["(cd a && uv sync)", "(cd b && uv sync)"]
    );
}

#[test]
fn secret_env_setup_left_out() {
    let wf = "\
jobs:
  test:
    steps:
      - working-directory: worker-service
        env: { TOKEN: ${{ secrets.T }} }
        run: uv sync
";
    let d = write_workflow(wf);
    assert!(detect_ci_setup(d.path()).is_empty());
    let left_out = detect_ci_left_out(d.path());
    assert_eq!(left_out.len(), 1);
    assert_eq!(left_out[0].0, "uv sync");
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
fn init_print_combines_setup() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("uv.lock"), "").unwrap();
    let wf = r.join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(
        &wf,
        "\
defaults:
  run:
    working-directory: worker-service
jobs:
  test:
    steps:
      - run: uv sync --frozen --all-groups
      - run: uv run --frozen --no-sync pytest
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
            "setup = \"uv sync && (cd worker-service && uv sync --frozen --all-groups)\""
        ),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
