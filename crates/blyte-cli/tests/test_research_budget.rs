//! Research runs default to 300 turns and 30 minutes; a run looped for 300 turns (2026-10-01).

use std::path::Path;
use std::process::Command;

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

fn start(r: &Path, args: &[&str], id: &str) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(r)
        .env("BLYTE_STATE_DIR", r.join(".state"))
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&std::fs::read_to_string(r.join(format!(".wt/{id}.start.json"))).unwrap())
        .unwrap()
}

#[test]
fn research_defaults_to_a_tight_budget_and_flags_still_win() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("q.md"), "Where is f.txt?\n").unwrap();
    // blyte.toml sizes implementation runs; research ignores its budget.
    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\nmax_turns = 300\nmax_seconds = 3600\n",
    )
    .unwrap();

    let s = start(
        r,
        &["dispatch", "Q1", "q.md", "--research", "--sandbox", "off"],
        "Q1",
    );
    assert_eq!(
        (s["max_turns"].as_u64(), s["max_seconds"].as_u64()),
        (Some(300), Some(1800))
    );

    let s = start(
        r,
        &[
            "dispatch",
            "Q2",
            "q.md",
            "--research",
            "--max-turns",
            "120",
            "--sandbox",
            "off",
        ],
        "Q2",
    );
    assert_eq!(s["max_turns"].as_u64(), Some(120));

    std::fs::write(r.join("t.md"), "x").unwrap();
    let s = start(r, &["dispatch", "T1", "t.md", "--sandbox", "off"], "T1");
    assert_eq!(
        (s["max_turns"].as_u64(), s["max_seconds"].as_u64()),
        (Some(300), Some(3600))
    );
}
