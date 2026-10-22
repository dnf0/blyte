//! A dispatch records its session in `.wt/<id>.start.json` (2026-10-07).

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

fn start_with_env(
    r: &Path,
    id: &str,
    blyte_session: Option<&str>,
    claude_session: Option<&str>,
) -> serde_json::Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.args([
        "dispatch",
        id,
        "task.md",
        "--gate",
        "true",
        "--sandbox",
        "off",
    ])
    .current_dir(r)
    .env("BLYTE_STATE_DIR", r.join(".state"))
    .env("BLYTE_NO_UPDATE_CHECK", "1")
    .env_remove("GEMINI_API_KEY")
    .env_remove("GOOGLE_API_KEY")
    .env_remove("BLYTE_MODEL");

    match blyte_session {
        Some(s) => {
            cmd.env("BLYTE_SESSION", s);
        }
        None => {
            cmd.env_remove("BLYTE_SESSION");
        }
    }

    match claude_session {
        Some(s) => {
            cmd.env("CLAUDE_CODE_SESSION_ID", s);
        }
        None => {
            cmd.env_remove("CLAUDE_CODE_SESSION_ID");
        }
    }

    let out = cmd.output().unwrap();
    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&std::fs::read_to_string(r.join(format!(".wt/{id}.start.json"))).unwrap())
        .unwrap()
}

#[test]
fn start_file_records_session_from_env() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();

    let s1 = start_with_env(r, "S1", Some("s-1"), None);
    assert_eq!(s1["session"], "s-1");

    let s2 = start_with_env(r, "S2", None, None);
    assert!(
        s2.get("session").is_none(),
        "expected no session key in {s2}"
    );

    let s3 = start_with_env(r, "S3", None, Some("claude-1"));
    assert_eq!(s3["session"], "claude-1");
}
