//! A repository says once how its work is checked (blyte.toml), and `blyte init` detects it:
//! in every skill eval the orchestrator invented the gate itself (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::{detect, load, render};

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap()
        .success());
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    let env_dir = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("HOME", env_dir.path())
        .env("XDG_CONFIG_HOME", env_dir.path())
        .env("BLYTE_STATE_DIR", env_dir.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn rust_is_detected_with_fmt_clippy_and_tests() {
    let r = tempfile::tempdir().unwrap();
    std::fs::write(r.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    assert_eq!(
        detect(r.path()),
        vec![
            "cargo fmt --all --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test --workspace --all-targets",
        ]
    );
}

#[test]
fn python_uses_uv_when_locked_and_only_the_tools_it_has() {
    let r = tempfile::tempdir().unwrap();
    std::fs::write(
        r.path().join("pyproject.toml"),
        "[project]\nname = \"x\"\n[tool.ruff]\nline-length = 100\n",
    )
    .unwrap();
    std::fs::write(r.path().join("uv.lock"), "").unwrap();
    std::fs::create_dir(r.path().join("tests")).unwrap();
    assert_eq!(
        detect(r.path()),
        vec!["uv run ruff check .", "uv run pytest -q"]
    );
    std::fs::remove_file(r.path().join("uv.lock")).unwrap();
    assert_eq!(
        detect(r.path()),
        vec!["python3 -m ruff check .", "python3 -m pytest -q"]
    );
}

#[test]
fn node_runs_the_scripts_it_defines_with_its_package_manager() {
    let r = tempfile::tempdir().unwrap();
    std::fs::write(
        r.path().join("package.json"),
        r#"{"scripts":{"test":"vitest","lint":"eslint ."}}"#,
    )
    .unwrap();
    std::fs::write(r.path().join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(detect(r.path()), vec!["pnpm run lint", "pnpm test"]);
}

#[test]
fn render_round_trips_through_load() {
    let r = tempfile::tempdir().unwrap();
    let steps = vec![
        "cargo fmt --all --check".to_string(),
        "cargo test".to_string(),
    ];
    std::fs::write(r.path().join("blyte.toml"), render(&steps)).unwrap();
    let cfg = load(r.path()).unwrap().unwrap();
    assert_eq!(
        cfg.dispatch.gate.as_deref(),
        Some("cargo fmt --all --check\ncargo test\n")
    );
    assert_eq!(cfg.dispatch.max_turns, Some(300));
}

#[test]
fn a_bad_key_is_an_error_that_names_the_file() {
    let r = tempfile::tempdir().unwrap();
    std::fs::write(r.path().join("blyte.toml"), "[dispatch]\ngaet = \"x\"\n").unwrap();
    let e = load(r.path()).unwrap_err();
    assert!(e.contains("blyte.toml"), "{e}");
}

#[test]
fn init_writes_once_and_refuses_to_overwrite() {
    let r = repo();
    std::fs::write(r.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let out = blyte(r.path(), &["init", "--no-verify"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(std::fs::read_to_string(r.path().join("blyte.toml"))
        .unwrap()
        .contains("cargo clippy"));
    assert_eq!(
        blyte(r.path(), &["init", "--no-verify"]).status.code(),
        Some(1)
    );
    assert_eq!(
        blyte(r.path(), &["init", "--force", "--no-verify"])
            .status
            .code(),
        Some(0)
    );
    let empty = repo();
    assert_eq!(
        blyte(empty.path(), &["init", "--no-verify"]).status.code(),
        Some(1)
    );
}

#[test]
fn dispatch_takes_its_gate_from_blyte_toml_and_says_so() {
    let r = repo();
    assert!(Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--allow-empty",
            "-qm",
            "i",
            "--no-gpg-sign"
        ])
        .current_dir(r.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap()
        .success());
    std::fs::write(r.path().join("task.md"), "x").unwrap();
    assert_eq!(
        blyte(r.path(), &["dispatch", "T1", "task.md", "--sandbox", "off"])
            .status
            .code(),
        Some(2)
    );
    std::fs::write(r.path().join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T2",
            "task.md",
            "--sandbox",
            "off",
            "--no-base-check",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(v["gate_source"], "blyte.toml");
}

#[test]
fn makefile_targets_win_over_guessing() {
    let r = tempfile::tempdir().unwrap();
    std::fs::write(
        r.path().join("pyproject.toml"),
        "[project]\nname = \"x\"\n[tool.ruff]\n",
    )
    .unwrap();
    std::fs::write(
        r.path().join("Makefile"),
        ".PHONY: lint test\nVENV := .venv\nlint:\n\truff check .\n\tpyright\ntest:\n\tpytest -q\n",
    )
    .unwrap();
    assert_eq!(detect(r.path()), vec!["make lint", "make test"]);
    std::fs::write(
        r.path().join("Makefile"),
        "check: lint test\nlint:\n\ttrue\n",
    )
    .unwrap();
    assert_eq!(detect(r.path()), vec!["make check"]);
}
