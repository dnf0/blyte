//! A dispatch can say which plan task it implements, so the dashboard can show plans (2026-10-01).

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

fn start(r: &Path, extra: &[&str], id: &str) -> serde_json::Value {
    let mut args = vec![
        "dispatch",
        id,
        "task.md",
        "--gate",
        "true",
        "--sandbox",
        "off",
    ];
    args.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(&args)
        .current_dir(r)
        .env("BLYTE_STATE_DIR", r.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
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
fn the_start_file_records_the_plan_task() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();

    let s = start(
        r,
        &[
            "--plan",
            "docs/plans/widget.md",
            "--plan-task",
            "Task 1: core parser",
        ],
        "P1",
    );
    assert_eq!(s["plan"], "docs/plans/widget.md");
    assert_eq!(s["plan_task"], "Task 1: core parser");

    let s = start(r, &[], "P2");
    assert!(s["plan"].is_null() && s["plan_task"].is_null(), "{s}");
}
