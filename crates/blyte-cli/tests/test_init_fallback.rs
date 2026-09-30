//! `blyte init` uses the checks the repository itself has when every CI step fails on HEAD (a stale
//! workflow), and a cold uv cache is an environment problem (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::load;
use blyte_cli::init::{classify, Verdict};

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

#[test]
fn a_cold_uv_cache_is_an_environment_problem_with_a_hint() {
    let out = "  × Failed to download `duckdb==1.5.4`\n  ╰─▶ Network connectivity is disabled, but the\n      requested data wasn't found in the cache for:\n";
    match classify(out, 1) {
        Verdict::Environment(reason) => assert!(reason.contains("uv sync"), "{reason}"),
        v => panic!("{v:?}"),
    }
}

#[test]
fn a_stale_ci_falls_back_to_the_detected_checks() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    // CI runs make targets that no longer exist; the Makefile's own `check` passes.
    write(
        &r.join(".github/workflows/ci.yml"),
        "jobs:\n  t:\n    steps:\n      - run: make lint\n      - run: make test\n",
    );
    write(&r.join("Makefile"), "check:\n\t@true\n");
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["init", "--sandbox", "off"])
        .current_dir(r)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        err.contains("trying the checks detected from the repository"),
        "{err}"
    );
    let gate = load(r).unwrap().unwrap().dispatch.gate.unwrap();
    assert_eq!(gate.trim(), "make check");
    let text = std::fs::read_to_string(r.join("blyte.toml")).unwrap();
    assert!(
        text.contains("# make check: from detected, passed"),
        "{text}"
    );
}
