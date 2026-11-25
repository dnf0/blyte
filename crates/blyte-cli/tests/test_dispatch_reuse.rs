//! Tests for `blyte dispatch <ID>` worktree reuse, running ID checks, and summary clearing.

use std::path::Path;
use std::process::Command;

fn blyte_bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
    }
    dir
}

fn run_dispatch_raw(cwd: &Path, id: &str, task: &str, args: &[&str]) -> std::process::Output {
    let home = cwd.join("_home");
    std::fs::create_dir_all(&home).unwrap();
    let task_file = cwd.join("task.md");
    std::fs::write(&task_file, task).unwrap();
    let mut cmd = Command::new(blyte_bin());
    cmd.args([
        "dispatch",
        id,
        task_file.to_str().unwrap(),
        "--sandbox",
        "off",
    ]);
    cmd.args(args);
    cmd.current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .output()
        .unwrap()
}

fn run_dispatch_with_args(
    cwd: &Path,
    id: &str,
    task: &str,
    extra: &[&str],
) -> std::process::Output {
    let mut args = vec!["--gate", "true"];
    args.extend_from_slice(extra);
    run_dispatch_raw(cwd, id, task, &args)
}

fn run_dispatch(cwd: &Path, id: &str, task: &str) -> std::process::Output {
    run_dispatch_with_args(cwd, id, task, &[])
}

fn archive_dirs_for(root: &Path, id: &str) -> Vec<std::path::PathBuf> {
    let archive_root = root.join(".wt").join("archive");
    if !archive_root.is_dir() {
        return Vec::new();
    }
    let prefix = format!("{id}-");
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&archive_root)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && entry.path().is_dir() {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect();
    dirs.sort();
    dirs
}

fn git_rev_parse(cwd: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn dispatch_refuses_to_reuse_worktree_from_older_base() {
    let r = init_repo();
    let root = r.path();

    let out1 = run_dispatch(root, "T1", "first task");
    assert_eq!(out1.status.code(), Some(1));

    let wt_dir = root.join(".wt").join("T1");
    assert!(wt_dir.is_dir());
    std::fs::write(wt_dir.join("t1.txt"), "unmerged\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "t1.txt"])
        .current_dir(&wt_dir)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "unmerged T1 work"])
        .current_dir(&wt_dir)
        .status()
        .unwrap()
        .success());
    let wt_head_before = git_rev_parse(&wt_dir, "HEAD");

    std::fs::write(root.join("new.txt"), "new content\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "new.txt"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "advance main"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    let new_main_head = git_rev_parse(root, "HEAD");

    let out2 = run_dispatch(root, "T1", "second task");
    assert_eq!(out2.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out2.stderr);
    let expected = format!(
        "error: .wt/T1 is from an earlier run based on {}, which does not contain {} (HEAD). Run `blyte clean`, remove it with `git worktree remove .wt/T1` and `git branch -D wip/T1` if it is not needed, or dispatch with a new ID.",
        &wt_head_before[..7],
        &new_main_head[..7],
    );
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\ngot:\n{stderr}"
    );
    assert!(wt_dir.is_dir(), "worktree should still exist");
    assert_eq!(
        git_rev_parse(&wt_dir, "HEAD"),
        wt_head_before,
        "worktree HEAD should be unchanged"
    );
}

#[test]
fn dispatch_reuses_worktree_when_base_has_not_moved() {
    let r = init_repo();
    let root = r.path();

    let out1 = run_dispatch(root, "T1", "first task");
    assert_eq!(out1.status.code(), Some(1));

    let out2 = run_dispatch(root, "T1", "retry task");
    assert_eq!(out2.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(root.join(".wt").join("T1.task.md")).unwrap(),
        "retry task"
    );
}

#[test]
fn dispatch_refuses_when_run_with_same_id_is_still_running() {
    let r = init_repo();
    let root = r.path();
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let start_json = serde_json::json!({
        "id": "T1",
        "pid": pid,
        "started_at": now,
    });
    std::fs::write(
        wt_dir.join("T1.start.json"),
        serde_json::to_string_pretty(&start_json).unwrap(),
    )
    .unwrap();

    let out = run_dispatch(root, "T1", "do task");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = format!(
        "error: run T1 is still running (pid {pid}). Wait for it to finish, or dispatch with a new ID."
    );
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\ngot:\n{stderr}"
    );
}

#[test]
fn dispatch_archives_earlier_records_including_verdict_for_cleaned_run() {
    let r = init_repo();
    let root = r.path();
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    let old_summary = r#"{"outcome":"verified"}"#;
    let old_verdict = r#"{"verdict":"accepted","notes":"old run","ts":1}"#;
    std::fs::write(wt_dir.join("T1.json"), old_summary).unwrap();
    std::fs::write(wt_dir.join("T1.verdict.json"), old_verdict).unwrap();
    std::fs::write(wt_dir.join("T1.report.json"), r#"{"outcome":"verified"}"#).unwrap();
    std::fs::write(wt_dir.join("T1.log"), "old log\n").unwrap();
    std::fs::write(wt_dir.join("T1.setup.log"), "old setup\n").unwrap();
    std::fs::write(wt_dir.join("T1.hold.json"), r#"{"reason":"hold"}"#).unwrap();
    std::fs::write(wt_dir.join("T1.removed.json"), r#"{"reason":"merged"}"#).unwrap();
    std::fs::write(wt_dir.join("T1.out"), "keep out\n").unwrap();

    let out = run_dispatch(root, "T1", "do task");
    assert_eq!(out.status.code(), Some(1));

    let summary_str = std::fs::read_to_string(wt_dir.join("T1.json")).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&summary_str).unwrap();
    assert_ne!(summary["outcome"], "verified");
    assert_eq!(summary["outcome"], "error");

    assert!(
        !wt_dir.join("T1.verdict.json").exists(),
        ".wt/T1.verdict.json must be archived out of .wt/"
    );
    assert_eq!(
        blyte_cli::queue_actions::read_verdict(root, "T1").unwrap(),
        None,
        "verdict reader must see no verdict for the new run"
    );
    assert_eq!(
        std::fs::read_to_string(wt_dir.join("T1.out")).unwrap(),
        "keep out\n"
    );

    let archives = archive_dirs_for(root, "T1");
    assert_eq!(archives.len(), 1, "expected one archive dir: {archives:?}");
    let archive = &archives[0];
    assert_eq!(
        std::fs::read_to_string(archive.join("T1.verdict.json")).unwrap(),
        old_verdict
    );
    assert_eq!(
        std::fs::read_to_string(archive.join("T1.json")).unwrap(),
        old_summary
    );
    assert!(archive.join("T1.report.json").is_file());
    assert!(archive.join("T1.log").is_file());
    assert!(archive.join("T1.setup.log").is_file());
    assert!(archive.join("T1.hold.json").is_file());
    assert!(archive.join("T1.removed.json").is_file());
    assert!(!archive.join("T1.out").exists());
    assert!(!archive.join("T1.task.md").exists());
}

#[test]
fn dispatch_reusing_worktree_on_same_base_archives_verdict() {
    let r = init_repo();
    let root = r.path();
    let wt_parent = root.join(".wt");

    let out1 = run_dispatch(root, "T1", "first task");
    assert_eq!(out1.status.code(), Some(1));

    let wt_dir = wt_parent.join("T1");
    std::fs::write(wt_dir.join("work.txt"), "unmerged\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "work.txt"])
        .current_dir(&wt_dir)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "unmerged T1 commit"])
        .current_dir(&wt_dir)
        .status()
        .unwrap()
        .success());
    let wt_head = git_rev_parse(&wt_dir, "HEAD");

    let old_verdict = r#"{"verdict":"accepted","notes":"earlier","ts":1}"#;
    std::fs::write(wt_parent.join("T1.verdict.json"), old_verdict).unwrap();
    std::fs::write(wt_parent.join("T1.checks.json"), r#"{"ok":true}"#).unwrap();

    let out2 = run_dispatch(root, "T1", "retry task");
    assert_eq!(out2.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out2.stderr);
    assert!(
        !stderr.contains("replaced the earlier run"),
        "worktree on same base should be reused, not replaced: {stderr}"
    );
    assert_eq!(git_rev_parse(&wt_dir, "HEAD"), wt_head);
    assert!(!wt_parent.join("T1.verdict.json").exists());
    assert_eq!(
        blyte_cli::queue_actions::read_verdict(root, "T1").unwrap(),
        None
    );

    let archives = archive_dirs_for(root, "T1");
    assert_eq!(archives.len(), 1, "expected one archive dir: {archives:?}");
    assert_eq!(
        std::fs::read_to_string(archives[0].join("T1.verdict.json")).unwrap(),
        old_verdict
    );
    assert!(archives[0].join("T1.checks.json").is_file());
}

#[test]
fn dispatch_research_archives_leftover_verdict_and_refuses_when_scratch_exists() {
    let r = init_repo();
    let root = r.path();
    let wt_parent = root.join(".wt");
    std::fs::create_dir_all(&wt_parent).unwrap();

    let old_verdict = r#"{"verdict":"accepted","notes":"old research","ts":1}"#;
    let old_start = r#"{"id":"R1","pid":99999999,"started_at":1}"#;
    std::fs::write(wt_parent.join("R1.verdict.json"), old_verdict).unwrap();
    std::fs::write(wt_parent.join("R1.report.md"), "old report\n").unwrap();
    std::fs::write(wt_parent.join("R1.start.json"), old_start).unwrap();
    std::fs::write(wt_parent.join("R1.out"), "keep out\n").unwrap();

    let out1 = run_dispatch_raw(root, "R1", "research task", &["--research"]);
    assert_eq!(
        out1.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(!wt_parent.join("R1.verdict.json").exists());
    assert_eq!(
        blyte_cli::queue_actions::read_verdict(root, "R1").unwrap(),
        None
    );
    assert_eq!(
        std::fs::read_to_string(wt_parent.join("R1.out")).unwrap(),
        "keep out\n"
    );

    let archives = archive_dirs_for(root, "R1");
    assert_eq!(archives.len(), 1, "expected one archive dir: {archives:?}");
    assert_eq!(
        std::fs::read_to_string(archives[0].join("R1.verdict.json")).unwrap(),
        old_verdict
    );
    assert!(archives[0].join("R1.report.md").is_file());
    assert!(archives[0].join("R1.start.json").is_file());
    assert!(!archives[0].join("R1.out").exists());

    std::fs::remove_dir_all(wt_parent.join("archive")).unwrap();
    let new_verdict = r#"{"verdict":"fixed","notes":"second","ts":2}"#;
    std::fs::write(wt_parent.join("R1.verdict.json"), new_verdict).unwrap();
    let start_before = std::fs::read_to_string(wt_parent.join("R1.start.json")).unwrap();

    let out2 = run_dispatch_raw(root, "R1", "research again", &["--research"]);
    assert_eq!(out2.status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(wt_parent.join("R1.verdict.json")).unwrap(),
        new_verdict
    );
    assert_eq!(
        std::fs::read_to_string(wt_parent.join("R1.start.json")).unwrap(),
        start_before
    );
    assert!(!wt_parent.join("archive").exists());
}

#[test]
fn dispatch_refusals_leave_earlier_records_untouched_and_create_no_archive() {
    let verdict_body = r#"{"verdict":"accepted","notes":"keep","ts":1}"#;
    let dead_start_body = r#"{"id":"X","pid":99999999,"started_at":1}"#;

    let r_old = init_repo();
    let root_old = r_old.path();
    let out1 = run_dispatch(root_old, "T1", "first task");
    assert_eq!(out1.status.code(), Some(1));
    let wt_t1 = root_old.join(".wt").join("T1");
    std::fs::write(wt_t1.join("a.txt"), "work\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "a.txt"])
        .current_dir(&wt_t1)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "unmerged"])
        .current_dir(&wt_t1)
        .status()
        .unwrap()
        .success());
    std::fs::write(root_old.join("main.txt"), "advance\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "main.txt"])
        .current_dir(root_old)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "advance main"])
        .current_dir(root_old)
        .status()
        .unwrap()
        .success());
    std::fs::write(root_old.join(".wt").join("T1.verdict.json"), verdict_body).unwrap();
    std::fs::write(root_old.join(".wt").join("T1.start.json"), dead_start_body).unwrap();
    let out_old = run_dispatch(root_old, "T1", "second");
    assert_eq!(out_old.status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(root_old.join(".wt").join("T1.verdict.json")).unwrap(),
        verdict_body
    );
    assert_eq!(
        std::fs::read_to_string(root_old.join(".wt").join("T1.start.json")).unwrap(),
        dead_start_body
    );
    assert!(!root_old.join(".wt").join("archive").exists());

    let r_branch = init_repo();
    let root_branch = r_branch.path();
    assert!(Command::new("git")
        .args(["checkout", "-q", "-b", "wip/U1"])
        .current_dir(root_branch)
        .status()
        .unwrap()
        .success());
    std::fs::write(root_branch.join("u1.txt"), "unmerged\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "u1.txt"])
        .current_dir(root_branch)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "u1 work"])
        .current_dir(root_branch)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(root_branch)
        .status()
        .unwrap()
        .success());
    std::fs::create_dir_all(root_branch.join(".wt")).unwrap();
    std::fs::write(
        root_branch.join(".wt").join("U1.verdict.json"),
        verdict_body,
    )
    .unwrap();
    std::fs::write(
        root_branch.join(".wt").join("U1.start.json"),
        dead_start_body,
    )
    .unwrap();
    let out_branch = run_dispatch(root_branch, "U1", "retry");
    assert_eq!(out_branch.status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(root_branch.join(".wt").join("U1.verdict.json")).unwrap(),
        verdict_body
    );
    assert_eq!(
        std::fs::read_to_string(root_branch.join(".wt").join("U1.start.json")).unwrap(),
        dead_start_body
    );
    assert!(!root_branch.join(".wt").join("archive").exists());

    let r_live = init_repo();
    let root_live = r_live.path();
    std::fs::create_dir_all(root_live.join(".wt")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let live_start = serde_json::json!({
        "id": "L1",
        "pid": std::process::id(),
        "started_at": now,
    })
    .to_string();
    std::fs::write(root_live.join(".wt").join("L1.start.json"), &live_start).unwrap();
    std::fs::write(root_live.join(".wt").join("L1.verdict.json"), verdict_body).unwrap();
    let out_live = run_dispatch(root_live, "L1", "overlap");
    assert_eq!(out_live.status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(root_live.join(".wt").join("L1.verdict.json")).unwrap(),
        verdict_body
    );
    assert_eq!(
        std::fs::read_to_string(root_live.join(".wt").join("L1.start.json")).unwrap(),
        live_start
    );
    assert!(!root_live.join(".wt").join("archive").exists());

    let r_base = init_repo();
    let root_base = r_base.path();
    std::fs::create_dir_all(root_base.join(".wt")).unwrap();
    std::fs::write(root_base.join(".wt").join("B1.verdict.json"), verdict_body).unwrap();
    std::fs::write(root_base.join(".wt").join("B1.start.json"), dead_start_body).unwrap();
    let out_base = run_dispatch_with_args(root_base, "B1", "bad base", &["--base", "no-such-ref"]);
    assert_eq!(out_base.status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(root_base.join(".wt").join("B1.verdict.json")).unwrap(),
        verdict_body
    );
    assert_eq!(
        std::fs::read_to_string(root_base.join(".wt").join("B1.start.json")).unwrap(),
        dead_start_body
    );
    assert!(!root_base.join(".wt").join("archive").exists());
}

#[test]
fn dispatch_same_second_archive_collision_picks_distinct_directory() {
    let r = init_repo();
    let root = r.path();
    let wt_parent = root.join(".wt");
    let archive_root = wt_parent.join("archive");
    std::fs::create_dir_all(&archive_root).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for ts in now..=(now + 10) {
        let existing_dir = archive_root.join(format!("C1-{ts}"));
        std::fs::create_dir_all(&existing_dir).unwrap();
        std::fs::write(existing_dir.join("C1.verdict.json"), "earlier archive").unwrap();
    }

    let fresh_verdict = r#"{"verdict":"accepted","notes":"second in same second","ts":2}"#;
    std::fs::write(wt_parent.join("C1.verdict.json"), fresh_verdict).unwrap();

    let out = run_dispatch(root, "C1", "collide task");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!wt_parent.join("C1.verdict.json").exists());

    let suffixed: Vec<_> = archive_dirs_for(root, "C1")
        .into_iter()
        .filter(|p| p.file_name().unwrap().to_string_lossy().ends_with("-2"))
        .collect();
    assert_eq!(
        suffixed.len(),
        1,
        "expected one -2 archive dir, got: {:?}",
        archive_dirs_for(root, "C1")
    );
    assert_eq!(
        std::fs::read_to_string(suffixed[0].join("C1.verdict.json")).unwrap(),
        fresh_verdict
    );
    for ts in now..=(now + 10) {
        assert_eq!(
            std::fs::read_to_string(
                archive_root
                    .join(format!("C1-{ts}"))
                    .join("C1.verdict.json")
            )
            .unwrap(),
            "earlier archive"
        );
    }
}

#[test]
fn dispatch_fails_with_exit_2_when_archive_root_is_blocked() {
    let r = init_repo();
    let root = r.path();
    let wt_parent = root.join(".wt");
    std::fs::create_dir_all(&wt_parent).unwrap();

    let archive_blocker = wt_parent.join("archive");
    std::fs::write(&archive_blocker, "not a directory").unwrap();
    let verdict_body = r#"{"verdict":"accepted","notes":"keep","ts":1}"#;
    std::fs::write(wt_parent.join("F1.verdict.json"), verdict_body).unwrap();

    let out = run_dispatch(root, "F1", "blocked archive");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&archive_blocker.display().to_string()),
        "stderr should name blocked archive path: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(wt_parent.join("F1.verdict.json")).unwrap(),
        verdict_body
    );
}

#[test]
fn dispatch_fails_on_unparsable_start_file() {
    let r = init_repo();
    let root = r.path();
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    let start_file = wt_dir.join("T1.start.json");
    std::fs::write(&start_file, "not json").unwrap();

    let out = run_dispatch(root, "T1", "do task");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("T1.start.json"),
        "stderr must name the start file, got: {stderr}"
    );
}
