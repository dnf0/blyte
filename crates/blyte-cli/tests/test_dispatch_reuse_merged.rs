use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    std::fs::write(d.path().join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    let home = dir.join("_home");
    std::fs::create_dir_all(&home).unwrap();
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env("HOME", &home)
        .output()
        .unwrap()
}

fn write_earlier_records(root: &Path, id: &str) {
    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    std::fs::write(
        wt_dir.join(format!("{id}.start.json")),
        r#"{"id":"A1","pid":999999,"started_at":1000}"#,
    )
    .unwrap();
    std::fs::write(
        wt_dir.join(format!("{id}.json")),
        r#"{"outcome":"verified"}"#,
    )
    .unwrap();
    std::fs::write(wt_dir.join(format!("{id}.report.json")), r#"{"turns":3}"#).unwrap();
    std::fs::write(
        wt_dir.join(format!("{id}.verdict.json")),
        r#"{"verdict":"accepted"}"#,
    )
    .unwrap();
    std::fs::write(wt_dir.join(format!("{id}.log")), "old log\n").unwrap();
}

fn find_archive_dir(root: &Path, id: &str) -> PathBuf {
    let archive_root = root.join(".wt/archive");
    let prefix = format!("{id}-");
    let mut matches: Vec<PathBuf> = std::fs::read_dir(&archive_root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one archive dir for {id}"
    );
    matches.pop().unwrap()
}

#[test]
fn dispatch_replaces_cherry_picked_earlier_run() {
    let r = repo();
    let root = r.path();
    let wt_path = root.join(".wt/A1");
    git(
        root,
        &["worktree", "add", "-b", "wip/A1", wt_path.to_str().unwrap()],
    );
    std::fs::write(wt_path.join("f.txt"), "from A1\n").unwrap();
    git(
        &wt_path,
        &["commit", "-qam", "feat: A1 change", "--no-gpg-sign"],
    );
    let a1_sha = git(&wt_path, &["rev-parse", "HEAD"]);
    write_earlier_records(root, "A1");

    git(root, &["cherry-pick", "-x", &a1_sha]);
    let new_base = git(root, &["rev-parse", "HEAD"]);
    assert_ne!(new_base, a1_sha);

    std::fs::write(root.join("task.md"), "New task.\n").unwrap();
    let out = blyte(root, &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "dispatch: replaced the earlier run A1 (merged); its records are in .wt/archive/A1-"
        ),
        "unexpected stderr: {stderr}"
    );

    let archive_dir = find_archive_dir(root, "A1");
    assert_eq!(
        std::fs::read_to_string(archive_dir.join("A1.json")).unwrap(),
        r#"{"outcome":"verified"}"#
    );
    assert_eq!(
        std::fs::read_to_string(archive_dir.join("A1.report.json")).unwrap(),
        r#"{"turns":3}"#
    );
    assert_eq!(
        std::fs::read_to_string(archive_dir.join("A1.start.json")).unwrap(),
        r#"{"id":"A1","pid":999999,"started_at":1000}"#
    );
    assert_eq!(
        std::fs::read_to_string(archive_dir.join("A1.verdict.json")).unwrap(),
        r#"{"verdict":"accepted"}"#
    );
    assert_eq!(
        std::fs::read_to_string(archive_dir.join("A1.log")).unwrap(),
        "old log\n"
    );
    assert!(
        !root.join(".wt/A1.verdict.json").exists(),
        "old verdict must be moved to archive"
    );

    let new_wt_head = git(&wt_path, &["rev-parse", "HEAD"]);
    assert_eq!(new_wt_head, new_base);
}

#[test]
fn dispatch_replaces_squash_merged_earlier_run() {
    let r = repo();
    let root = r.path();
    let wt_path = root.join(".wt/A1");
    git(
        root,
        &["worktree", "add", "-b", "wip/A1", wt_path.to_str().unwrap()],
    );
    std::fs::write(wt_path.join("squash.txt"), "squash work\n").unwrap();
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: squash subject", "--no-gpg-sign"],
    );
    let a1_sha = git(&wt_path, &["rev-parse", "HEAD"]);
    write_earlier_records(root, "A1");

    // Squash commit on main whose message lists the run's commit subject, and changes another line
    std::fs::write(root.join("squash.txt"), "squash work tweaked on main\n").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "PR #42\n\n* feat: squash subject",
            "--no-gpg-sign",
        ],
    );
    let new_base = git(root, &["rev-parse", "HEAD"]);
    assert_ne!(new_base, a1_sha);

    std::fs::write(root.join("task.md"), "New task.\n").unwrap();
    let out = blyte(root, &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "dispatch: replaced the earlier run A1 (merged); its records are in .wt/archive/A1-"
        ),
        "unexpected stderr: {stderr}"
    );

    let archive_dir = find_archive_dir(root, "A1");
    assert!(archive_dir.join("A1.json").is_file());
    assert!(archive_dir.join("A1.start.json").is_file());
    let new_wt_head = git(&wt_path, &["rev-parse", "HEAD"]);
    assert_eq!(new_wt_head, new_base);
}

#[test]
fn dispatch_keeps_unmerged_earlier_run_untouched() {
    let r = repo();
    let root = r.path();
    let wt_path = root.join(".wt/A1");
    git(
        root,
        &["worktree", "add", "-b", "wip/A1", wt_path.to_str().unwrap()],
    );
    std::fs::write(wt_path.join("unmerged.txt"), "unmerged\n").unwrap();
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "feat: unmerged", "--no-gpg-sign"],
    );
    let a1_sha = git(&wt_path, &["rev-parse", "HEAD"]);
    write_earlier_records(root, "A1");

    // Also test the branch-only unmerged case ("is not merged" message) and worktree on older base:
    // First remove the worktree so branch-only unmerged message is checked, or check both!
    git(root, &["worktree", "remove", ".wt/A1"]);

    std::fs::write(root.join("task.md"), "New task.\n").unwrap();
    let out = blyte(root, &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists from an earlier run and is not merged"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(git(root, &["rev-parse", "refs/heads/wip/A1"]), a1_sha);
    assert_eq!(
        std::fs::read_to_string(root.join(".wt/A1.json")).unwrap(),
        r#"{"outcome":"verified"}"#
    );
    assert!(!root.join(".wt/archive").exists());

    // Re-add the worktree and advance main so `.wt/A1` is also present with unmerged commits:
    git(root, &["worktree", "add", ".wt/A1", "wip/A1"]);
    std::fs::write(root.join("main_advance.txt"), "main\n").unwrap();
    git(root, &["add", "main_advance.txt"]);
    git(root, &["commit", "-qm", "advance main", "--no-gpg-sign"]);

    let out2 = blyte(root, &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(
        out2.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(wt_path.is_dir());
    assert_eq!(git(&wt_path, &["rev-parse", "HEAD"]), a1_sha);
    assert_eq!(
        std::fs::read_to_string(root.join(".wt/A1.json")).unwrap(),
        r#"{"outcome":"verified"}"#
    );
    assert!(!root.join(".wt/archive").exists());
}

#[test]
fn dispatch_refuses_merged_run_with_uncommitted_changes() {
    let r = repo();
    let root = r.path();
    let wt_path = root.join(".wt/A1");
    git(
        root,
        &["worktree", "add", "-b", "wip/A1", wt_path.to_str().unwrap()],
    );
    std::fs::write(wt_path.join("f.txt"), "from A1\n").unwrap();
    git(
        &wt_path,
        &["commit", "-qam", "feat: A1 change", "--no-gpg-sign"],
    );
    let a1_sha = git(&wt_path, &["rev-parse", "HEAD"]);
    write_earlier_records(root, "A1");

    git(root, &["cherry-pick", "-x", &a1_sha]);

    // Leave uncommitted changes in the worktree
    std::fs::write(wt_path.join("dirty.txt"), "uncommitted\n").unwrap();

    std::fs::write(root.join("task.md"), "New task.\n").unwrap();
    let out = blyte(root, &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(wt_path.join("dirty.txt").is_file());
    assert_eq!(git(&wt_path, &["rev-parse", "HEAD"]), a1_sha);
    assert_eq!(
        std::fs::read_to_string(root.join(".wt/A1.json")).unwrap(),
        r#"{"outcome":"verified"}"#
    );
    assert!(!root.join(".wt/archive").exists());
}

#[test]
fn dispatch_replaces_merged_branch_when_worktree_already_gone() {
    let r = repo();
    let root = r.path();
    git(root, &["checkout", "-qb", "wip/A1"]);
    std::fs::write(root.join("f.txt"), "from A1\n").unwrap();
    git(
        root,
        &["commit", "-qam", "feat: A1 branch", "--no-gpg-sign"],
    );
    let a1_sha = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "main"]);
    git(root, &["cherry-pick", "-x", &a1_sha]);
    let new_base = git(root, &["rev-parse", "HEAD"]);
    write_earlier_records(root, "A1");

    std::fs::write(root.join("task.md"), "New task.\n").unwrap();
    let out = blyte(root, &["dispatch", "A1", "task.md", "--sandbox", "off"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "dispatch: replaced the earlier run A1 (merged); its records are in .wt/archive/A1-"
        ),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(git(root, &["rev-parse", "refs/heads/wip/A1"]), new_base);
    let archive_dir = find_archive_dir(root, "A1");
    assert!(archive_dir.join("A1.json").is_file());
}

#[test]
fn dispatch_preserves_task_md_written_before_dispatch() {
    let r = repo();
    let root = r.path();
    let wt_path = root.join(".wt/A1");
    git(
        root,
        &["worktree", "add", "-b", "wip/A1", wt_path.to_str().unwrap()],
    );
    std::fs::write(wt_path.join("f.txt"), "from A1\n").unwrap();
    git(
        &wt_path,
        &["commit", "-qam", "feat: A1 change", "--no-gpg-sign"],
    );
    let a1_sha = git(&wt_path, &["rev-parse", "HEAD"]);
    write_earlier_records(root, "A1");
    git(root, &["cherry-pick", "-x", &a1_sha]);

    let task_path = root.join(".wt/A1.task.md");
    std::fs::write(&task_path, "Caller task in .wt/A1.task.md\n").unwrap();

    let out = blyte(
        root,
        &[
            "dispatch",
            "A1",
            task_path.to_str().unwrap(),
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&task_path).unwrap(),
        "Caller task in .wt/A1.task.md\n"
    );
    let archive_dir = find_archive_dir(root, "A1");
    assert!(!archive_dir.join("A1.task.md").exists());
}
