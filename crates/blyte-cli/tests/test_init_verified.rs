//! `blyte init` takes the gate from CI, sets what the environment needs, and proves each step in a
//! fresh worktree before writing it: it had picked `make check` (docs only) and a `make test`
//! that needed credentials (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::load;
use blyte_cli::init::{
    classify, detect_ci, detect_env, detect_setup, render_verified, InitStep, Verdict,
};

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

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const WORKFLOW: &str = "\
name: PR
jobs:
  test:
    steps:
      - uses: astral-sh/setup-uv@v5
      - name: Install
        run: uv sync --group dev
      - name: Lint
        run: uv run ruff check .
      - name: Tests
        run: | # pipefail so pytest's failure counts
          set -o pipefail
          uv run pytest -m \"not e2e\" --cov=pkg \\
            --cov-report=xml --junitxml=out.xml | tee out.txt
      - name: Lint again
        run: uv run ruff check .
";

#[test]
fn ci_steps_are_the_gate_without_setup_or_reporting() {
    let d = tempfile::tempdir().unwrap();
    write(
        &d.path().join(".github/workflows/pull_request.yaml"),
        WORKFLOW,
    );
    assert_eq!(
        detect_ci(d.path()),
        vec!["uv run ruff check .", "uv run pytest -m \"not e2e\""]
    );
}

#[test]
fn uv_projects_run_frozen_and_private_indexes_offline() {
    let d = tempfile::tempdir().unwrap();
    write(&d.path().join("uv.lock"), "");
    write(
        &d.path().join("pyproject.toml"),
        "[project]\nname = \"x\"\n",
    );
    let env = detect_env(d.path());
    assert_eq!(env.get("UV_FROZEN").map(String::as_str), Some("1"));
    assert!(!env.contains_key("UV_OFFLINE"));
    write(
        &d.path().join("pyproject.toml"),
        "[project]\nname = \"x\"\n[[tool.uv.index]]\nname = \"p\"\nurl = \"https://packages.example.com/simple/\"\n",
    );
    assert_eq!(
        detect_env(d.path()).get("UV_OFFLINE").map(String::as_str),
        Some("1")
    );
    assert!(detect_env(tempfile::tempdir().unwrap().path()).is_empty());
}

#[test]
fn failures_are_told_apart_from_environment_problems() {
    assert_eq!(classify("ok", 0), Verdict::Passed);
    assert!(matches!(
        classify("error: Failed to fetch: 401 Unauthorized\n", 1),
        Verdict::Environment(_)
    ));
    assert!(matches!(
        classify(
            "× Because cursor-agents was not found in the package registry\n",
            1
        ),
        Verdict::Environment(_)
    ));
    assert_eq!(
        classify(
            "FAILED tests/test_x.py::test_y - AssertionError\n1 failed\n",
            1
        ),
        Verdict::Failed
    );
}

#[test]
fn init_proves_each_step_and_leaves_out_the_ones_the_environment_breaks() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    write(
        &r.join(".github/workflows/ci.yml"),
        "jobs:\n  t:\n    steps:\n      - run: make check\n      - run: make lint\n      - run: make test\n",
    );
    write(
        &r.join("Makefile"),
        "check:\n\t@true\nlint:\n\t@echo 'error: 401 Unauthorized' >&2; exit 1\ntest:\n\t@echo '1 failed'; exit 1\n",
    );
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["init", "--sandbox", "off"])
        .current_dir(r)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(r.join("blyte.toml")).unwrap();
    assert!(text.contains("# make check: from ci.yml, passed"), "{text}");
    assert!(
        text.contains("# make lint: from ci.yml, left out: "),
        "{text}"
    );
    assert!(
        text.contains("# make test: from ci.yml, failed on HEAD"),
        "{text}"
    );
    let gate = load(r).unwrap().unwrap().dispatch.gate.unwrap();
    assert_eq!(
        gate.lines().collect::<Vec<_>>(),
        vec!["make check", "make test"]
    );
    // The verification worktree is gone.
    let wts = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(r)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&wts.stdout).lines().count(), 1);
}

#[test]
fn detect_setup_proposes_uv_sync_when_locked() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(detect_setup(d.path()), None);
    write(&d.path().join("uv.lock"), "");
    assert_eq!(detect_setup(d.path()), Some("uv sync".to_string()));
}

#[test]
fn render_verified_handles_setup_states() {
    let steps = vec![InitStep {
        command: "cargo test".to_string(),
        source: "ci.yml".to_string(),
        verdict: Some(Verdict::Passed),
    }];
    let env = std::collections::BTreeMap::new();

    // Case 1: passed renders setup = "uv sync" before gate with comment
    let passed_text = render_verified(&steps, &env, Some("uv sync"), true);
    assert!(
        passed_text.contains(
            "[dispatch]\n# prepares each new worktree before the implementer starts\nsetup = \"uv sync\"\ngate = \"\"\""
        ),
        "{passed_text}"
    );
    let d1 = tempfile::tempdir().unwrap();
    write(&d1.path().join("blyte.toml"), &passed_text);
    let cfg1 = load(d1.path()).unwrap().unwrap();
    assert_eq!(cfg1.dispatch.setup, Some("uv sync".to_string()));

    // Case 2: failed renders setup commented out with reason
    let failed_text = render_verified(&steps, &env, Some("uv sync"), false);
    assert!(
        failed_text.contains(
            "[dispatch]\n# setup = \"uv sync\"   # left out: failed in a fresh worktree\ngate = \"\"\""
        ),
        "{failed_text}"
    );
    let d2 = tempfile::tempdir().unwrap();
    write(&d2.path().join("blyte.toml"), &failed_text);
    let cfg2 = load(d2.path()).unwrap().unwrap();
    assert_eq!(cfg2.dispatch.setup, None);

    // Case 3: None renders neither
    let none_text = render_verified(&steps, &env, None, false);
    assert!(
        none_text.contains("[dispatch]\ngate = \"\"\""),
        "{none_text}"
    );
    assert!(!none_text.contains("setup"));
    let d3 = tempfile::tempdir().unwrap();
    write(&d3.path().join("blyte.toml"), &none_text);
    let cfg3 = load(d3.path()).unwrap().unwrap();
    assert_eq!(cfg3.dispatch.setup, None);
}

#[test]
fn init_leaves_out_failed_setup_with_warning() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    write(
        &r.join(".github/workflows/ci.yml"),
        "jobs:\n  t:\n    steps:\n      - run: make check\n",
    );
    write(&r.join("Makefile"), "check:\n\t@true\n");
    // Empty uv.lock causes `uv sync` to fail because there is no pyproject.toml
    write(&r.join("uv.lock"), "");
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["init", "--sandbox", "off"])
        .current_dir(r)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: setup failed; left out of blyte.toml:"),
        "{stderr}"
    );
    let text = std::fs::read_to_string(r.join("blyte.toml")).unwrap();
    assert!(
        text.contains("# setup = \"uv sync\"   # left out: failed in a fresh worktree"),
        "{text}"
    );
    let cfg = load(r).unwrap().unwrap();
    assert_eq!(cfg.dispatch.setup, None);
    assert_eq!(cfg.dispatch.gate.as_deref(), Some("make check\n"));
}
