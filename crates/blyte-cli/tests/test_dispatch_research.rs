//! Research is a dispatch: a scratch workspace under .wt/, a report gate, a `research` tag
//! (2026-10-01).

use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::{wrap_research, RESEARCH_RULES};

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

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_STATE_DIR", dir.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn the_research_prompt_is_read_only_and_names_the_code() {
    let p = wrap_research("  How is the gate run?\n", Path::new("/repo/x"));
    assert!(p.starts_with("READ-ONLY RESEARCH TASK. The code to research is under /repo/x"));
    assert!(p.contains("How is the gate run?"));
    assert!(p.ends_with(RESEARCH_RULES));
    assert!(RESEARCH_RULES.contains("report.md"));
}

#[test]
fn a_research_dispatch_uses_a_scratch_repository_and_a_report_gate() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("q.md"), "Where is f.txt written?\n").unwrap();

    // No key: the run fails fast, after the workspace and start file exist.
    let out = blyte(
        r,
        &["dispatch", "Q1", "q.md", "--research", "--sandbox", "off"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(v["gate_source"], "research");
    assert!(v["research_report"].is_null());
    let scratch = r.join(".wt/Q1");
    assert!(
        scratch.join(".git").is_dir(),
        "a scratch repository of its own"
    );
    let wts = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(r)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&wts.stdout).lines().count(),
        1,
        "not a worktree"
    );
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/Q1.start.json")).unwrap())
            .unwrap();
    assert!(start["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "research"));
    assert_eq!(start["research"], true);
    assert_eq!(std::fs::read_to_string(r.join("f.txt")).unwrap(), "a\n");

    // The same id again is a clash; --research with --gate is a usage error.
    assert_eq!(
        blyte(
            r,
            &["dispatch", "Q1", "q.md", "--research", "--sandbox", "off"]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        blyte(
            r,
            &["dispatch", "Q2", "q.md", "--research", "--gate", "true"]
        )
        .status
        .code(),
        Some(2)
    );
}
