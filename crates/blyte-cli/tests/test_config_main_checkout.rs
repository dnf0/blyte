//! A linked worktree has no untracked blyte.toml of its own; dispatch uses the main checkout's
//! (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::{load_with_fallback, main_checkout};

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
fn a_linked_worktree_falls_back_to_the_main_checkout() {
    let t = tempfile::tempdir().unwrap();
    let main = t.path().join("repo");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("f.txt"), "a\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "i", "--no-gpg-sign"]);
    // Untracked, as with a global ignore.
    std::fs::write(main.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    let linked = t.path().join("repo--feature");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );

    assert_eq!(
        main_checkout(&linked).map(|p| p.canonicalize().unwrap()),
        Some(main.canonicalize().unwrap())
    );
    assert_eq!(main_checkout(&main), None);

    let (cfg, from_main) = load_with_fallback(&linked).unwrap().expect("a config");
    assert!(from_main);
    assert_eq!(cfg.dispatch.gate.as_deref(), Some("true"));
    let (_, from_main) = load_with_fallback(&main).unwrap().expect("a config");
    assert!(!from_main);

    std::fs::write(linked.join("task.md"), "x").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "L1", "task.md", "--sandbox", "off"])
        .current_dir(&linked)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(v["gate_source"], "blyte.toml (main checkout)");
}
