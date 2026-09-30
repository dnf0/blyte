//! blyte.toml carries a repository's environment settings and fold budget, the prompt names the
//! gate's quick steps, and dispatch keeps its summary beside the run (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::load;
use blyte_cli::dispatch::quick_steps;

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
fn env_and_fold_budget_load_and_credentials_are_refused() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\nfold_budget = 40000\n\n[dispatch.env]\nUV_FROZEN = \"1\"\nUV_OFFLINE = \"1\"\n",
    )
    .unwrap();
    let cfg = load(d.path()).unwrap().unwrap().dispatch;
    assert_eq!(cfg.fold_budget, Some(40000));
    assert_eq!(cfg.env.get("UV_FROZEN").map(String::as_str), Some("1"));
    assert_eq!(cfg.env.len(), 2);

    for bad in [
        "GITHUB_TOKEN = \"x\"",
        "UV_INDEX_PIGEONHOLE_PASSWORD = \"x\"",
        "BLYTE_MODEL = \"x\"",
    ] {
        std::fs::write(
            d.path().join("blyte.toml"),
            format!("[dispatch]\ngate = \"true\"\n[dispatch.env]\n{bad}\n"),
        )
        .unwrap();
        let err = load(d.path()).unwrap_err();
        let name = bad.split(' ').next().unwrap();
        assert!(err.contains(name) && err.contains("blyte.toml"), "{err}");
    }
}

#[test]
fn quick_steps_are_every_gate_line_but_the_last() {
    assert_eq!(
        quick_steps("cargo fmt --all --check\ncargo clippy -- -D warnings\n\ncargo test\n")
            .as_deref(),
        Some(
            "The check's quick steps, to run yourself before you stop: `cargo fmt --all --check`, \
             `cargo clippy -- -D warnings`. The last step, the full run, happens at the gate."
        )
    );
    assert_eq!(quick_steps("pytest -q"), None);
}

#[test]
fn dispatch_saves_its_summary_and_records_env_names() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();
    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\n[dispatch.env]\nUV_FROZEN = \"1\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "E1", "task.md", "--sandbox", "off"])
        .current_dir(r)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    let printed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/E1.json")).unwrap()).unwrap();
    assert_eq!(printed, saved);
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/E1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["env"], serde_json::json!(["UV_FROZEN"]));
}
