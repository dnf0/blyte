//! `blyte dispatch` writes `.wt/<id>.start.json` before creating `.wt/<id>`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

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

fn setup_repo(r: &Path) {
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();
}

fn run_dispatch(r: &Path, id: &str, extra: &[&str]) -> Output {
    let mut args = vec!["dispatch", id, "task.md", "--sandbox", "off"];
    if !extra.contains(&"--research") {
        args.extend_from_slice(&["--gate", "true"]);
    }
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(&args)
        .current_dir(r)
        .env("BLYTE_STATE_DIR", r.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn the_start_record_exists_before_the_worktree() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    setup_repo(r);

    let hooks_dir = r.join(".git/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook = hooks_dir.join("post-checkout");
    std::fs::write(
        &hook,
        "#!/bin/sh\ntop=\"$(git rev-parse --git-common-dir)/..\"\nif [ -f \"$top/.wt/S1.start.json\" ]; then cp \"$top/.wt/S1.start.json\" \"$top/seen-start.json\"; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = run_dispatch(r, "S1", &[]);
    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let seen_path = r.join("seen-start.json");
    assert!(seen_path.exists(), "seen-start.json was not written");
    let seen: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seen_path).unwrap()).unwrap();
    assert_eq!(seen["starting"], serde_json::json!(true), "{seen}");
    assert_eq!(seen["id"], serde_json::json!("S1"), "{seen}");
    let seen_pid = seen["pid"].as_i64().expect("seen pid is integer");
    assert!(seen_pid > 0, "seen pid {seen_pid} <= 0");

    let final_rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/S1.start.json")).unwrap())
            .unwrap();
    assert!(
        final_rec.get("starting").is_none(),
        "final record still has starting: {final_rec}"
    );
    let final_pid = final_rec["pid"].as_i64().expect("final pid is integer");
    assert_ne!(final_pid, seen_pid, "final pid should differ from seen pid");
    assert_eq!(final_rec["started_at"], seen["started_at"]);
}

#[test]
fn a_failed_start_leaves_no_start_record() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    setup_repo(r);

    let out = run_dispatch(r, "S2", &["--base", "no-such-ref"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!r.join(".wt/S2.start.json").exists());
    assert!(!r.join(".wt/S2").exists());
}

#[test]
fn a_research_run_ends_with_a_complete_start_record() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    setup_repo(r);

    let out = run_dispatch(r, "S3", &["--research"]);
    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/S3.start.json")).unwrap())
            .unwrap();
    assert_eq!(rec["research"], serde_json::json!(true), "{rec}");
    assert!(rec.get("starting").is_none(), "{rec}");
    assert!(rec.get("gate").is_some(), "{rec}");
    assert!(rec.get("max_turns").is_some(), "{rec}");
    assert!(rec.get("max_seconds").is_some(), "{rec}");
}
