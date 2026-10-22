//! Tests for `[dispatch].allow_read` in blyte.toml.

use std::path::{Path, PathBuf};
use std::process::Command;

use blyte_cli::config::{expand_allow_read, load};

#[test]
fn test_expand_allow_read_expansion_and_order() {
    let entries = vec![
        "~/x".to_string(),
        "~".to_string(),
        "$BLYTE_MAIN_CHECKOUT/.venv".to_string(),
        "$BLYTE_MAIN_CHECKOUT".to_string(),
        "/abs/p".to_string(),
    ];
    let expanded = expand_allow_read(&entries, Some(Path::new("/h")), Path::new("/m")).unwrap();
    assert_eq!(
        expanded,
        vec![
            PathBuf::from("/h/x"),
            PathBuf::from("/h"),
            PathBuf::from("/m/.venv"),
            PathBuf::from("/m"),
            PathBuf::from("/abs/p"),
        ]
    );
}

#[test]
fn test_expand_allow_read_errors() {
    let err = expand_allow_read(
        &["rel/p".to_string()],
        Some(Path::new("/h")),
        Path::new("/m"),
    )
    .unwrap_err();
    assert!(
        err.contains("rel/p"),
        "expected error to contain `rel/p`, got: {err}"
    );

    let err_home_none = expand_allow_read(&["~/x".to_string()], None, Path::new("/m")).unwrap_err();
    assert!(
        err_home_none.contains("~/x"),
        "expected error to contain `~/x`, got: {err_home_none}"
    );
}

#[test]
fn test_load_allow_read() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("blyte.toml"),
        "[dispatch]\nallow_read = [\"/abs/p\"]\n",
    )
    .unwrap();
    let cfg = load(tmp.path()).unwrap().expect("should load config");
    assert_eq!(cfg.dispatch.allow_read, vec!["/abs/p"]);

    let tmp_rel = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp_rel.path().join("blyte.toml"),
        "[dispatch]\nallow_read = [\"rel\"]\n",
    )
    .unwrap();
    let err = load(tmp_rel.path()).unwrap_err();
    assert!(
        err.contains("rel"),
        "expected error to contain `rel`, got: {err}"
    );
    assert!(
        err.contains("blyte.toml"),
        "expected error to contain `blyte.toml`, got: {err}"
    );

    let tmp_empty = tempfile::tempdir().unwrap();
    std::fs::write(tmp_empty.path().join("blyte.toml"), "[dispatch]\n").unwrap();
    let cfg_empty = load(tmp_empty.path()).unwrap().expect("should load config");
    assert!(cfg_empty.dispatch.allow_read.is_empty());
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
fn test_dispatch_passes_allow_read_from_toml_and_cli() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();
    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\nallow_read = [\"/abs/toml\"]\n",
    )
    .unwrap();
    let _ = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "AR1",
            "task.md",
            "--sandbox",
            "off",
            "--allow-read",
            "/abs/cli",
        ])
        .current_dir(r)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    let report_path = r.join(".wt/AR1.report.json");
    if report_path.exists() {
        let rep: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
        assert_eq!(
            rep["allow_read"],
            serde_json::json!(["/abs/toml", "/abs/cli"])
        );
    }
}
