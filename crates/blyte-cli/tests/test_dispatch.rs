//! `blyte dispatch` runs one task in its own worktree and prints a JSON summary for the
//! orchestrator; `blyte review` shows what it did. They replace examples/orchestrate's bash
//! scripts (2026-09-27). No test here needs a model: a run without an API key ends `error`.

use std::path::Path;
use std::process::Command;

use blyte_cli::cli::{Cli, Command as Sub};
use blyte_cli::dispatch::{dispatch_outcome, exit_code, wrap_task, DISPATCH_RULES};
use clap::Parser;

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

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn subcommands_parse_and_a_plain_prompt_is_unchanged() {
    let cli = Cli::try_parse_from(["blyte", "dispatch", "T1", "task.md", "--gate", "cargo test"])
        .unwrap();
    match cli.command {
        Some(Sub::Dispatch(a)) => {
            assert_eq!(a.id, "T1");
            assert_eq!(a.gate.as_deref(), Some("cargo test"));
            assert_eq!(a.max_turns, None);
            assert_eq!(a.max_seconds, None);
        }
        other => panic!("expected dispatch, got {other:?}"),
    }
    assert!(
        Cli::try_parse_from(["blyte", "dispatch", "T1", "task.md"]).is_ok(),
        "--gate is optional with blyte.toml"
    );
    let plain = Cli::try_parse_from(["blyte", "fix the lexer"]).unwrap();
    assert!(plain.command.is_none());
    assert_eq!(plain.prompt.as_deref(), Some("fix the lexer"));
    let escaped = Cli::try_parse_from(["blyte", "--", "dispatch"]).unwrap();
    assert!(escaped.command.is_none());
    assert_eq!(escaped.prompt.as_deref(), Some("dispatch"));
}

#[test]
fn outcomes_and_exit_codes() {
    assert_eq!(dispatch_outcome("verified", 2), "verified");
    assert_eq!(dispatch_outcome("verified", 0), "no_changes");
    assert_eq!(dispatch_outcome("gate_failed", 0), "gate_failed");
    assert_eq!(dispatch_outcome("weird", 1), "error");
    assert_eq!(exit_code("verified"), 0);
    assert_eq!(exit_code("no_changes"), 4);
    assert_eq!(exit_code("gate_failed"), 1);
}

#[test]
fn the_task_is_wrapped_with_the_rules() {
    let p = wrap_task("  Append b to f.txt.\n");
    assert!(
        p.starts_with("You are implementing exactly one task."),
        "{p}"
    );
    assert!(p.contains("\n\nAppend b to f.txt.\n\n"), "{p}");
    assert!(p.ends_with(DISPATCH_RULES), "{p}");
}

#[test]
fn a_dispatch_makes_a_worktree_and_reports_json_even_when_the_run_fails() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "Append b to f.txt.\n").unwrap();
    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON object");
    assert_eq!(v["id"], "T1");
    assert_eq!(v["branch"], "wip/T1");
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["exit"], 1);
    assert_eq!(v["commits"], serde_json::json!([]));
    assert!(
        v["error"].as_str().unwrap_or("").contains("API key")
            || v["error"].as_str().unwrap_or("").contains("GEMINI_API_KEY"),
        "{v}"
    );
    assert!(r.path().join(".wt/T1/f.txt").exists());
    assert!(r.path().join(".wt/T1.log").exists());
    let exclude = std::fs::read_to_string(r.path().join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == ".wt/"), "{exclude}");
    // A second dispatch under the same id reuses the worktree.
    let again = blyte(
        r.path(),
        &[
            "dispatch",
            "T1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(again.status.code(), Some(1));
}

#[test]
fn review_shows_the_run() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "x").unwrap();
    blyte(
        r.path(),
        &[
            "dispatch",
            "T2",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    let out = blyte(r.path(), &["review", "T2"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    for s in ["== commits", "== files", "== tree", "(clean)"] {
        assert!(text.contains(s), "{s} missing from:\n{text}");
    }
    assert_eq!(blyte(r.path(), &["review", "nope"]).status.code(), Some(2));
}

#[test]
fn summary_carries_model_from_report() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "Append b to f.txt.\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "T1",
            "task.md",
            "--gate",
            "true",
            "--model",
            "gemini-4-argon",
            "--max-turns",
            "0",
            "--sandbox",
            "off",
        ])
        .current_dir(r.path())
        .env("GEMINI_API_KEY", "test-key")
        .env("BLYTE_USAGE_LOG", "off")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rep: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/T1.report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rep["model_id"], "gemini/gemini-4-argon");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON object");
    assert_eq!(v["model"], "gemini/gemini-4-argon");
}
